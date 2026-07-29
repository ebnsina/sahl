/**
 * The typed bridge to the till.
 *
 * Every function here is a thin wrapper over a Tauri command. **None of them compute anything.**
 * Money arrives as exact integers of minor units already calculated by `sahl-core`, and the UI's
 * only job is to hand those to `Intl.NumberFormat`. If you find yourself wanting to add two amounts
 * in this file, the operation belongs in Rust.
 *
 * There is deliberately no browser fallback that simulates a till. A mock would be a second
 * implementation of the money rules in TypeScript — exactly the drift the architecture exists to
 * prevent — so outside the Tauri shell these calls fail loudly and the UI renders a designed
 * "no till" state instead.
 */

import { invoke } from '@tauri-apps/api/core';

export interface LineView {
	id: string;
	name: string;
	/** Thousandths of a unit: 1234 is 1.234 kg. */
	quantityMilli: number;
	unitPriceMinor: number;
	totalMinor: number;
	taxMinor: number;
	voided: boolean;
}

export interface TaxGroupView {
	basisPoints: number;
	class: 'standard' | 'zero_rated' | 'exempt';
	taxableBaseMinor: number;
	taxMinor: number;
}

export interface TenderView {
	id: string;
	method: string;
	amountMinor: number;
}

export interface SaleView {
	id: string;
	status: 'open' | 'completed' | 'abandoned';
	currency: string;
	lines: LineView[];
	taxGroups: TaxGroupView[];
	tenders: TenderView[];
	grossMinor: number;
	discountMinor: number;
	netMinor: number;
	taxMinor: number;
	totalMinor: number;
	tenderedMinor: number;
	balanceDueMinor: number;
	changeDueMinor: number;
	voidCount: number;
	needsDrawer: boolean;
}

/**
 * A shift as the close-out screen shows it.
 *
 * `expectedCashMinor` is zero on a blind count sheet — see `blindCountSheet`. Everything here is an
 * exact integer from Rust; nothing on this screen adds two of them together.
 */
export interface ShiftView {
	id: string;
	cashier: string;
	/** False while the shift runs — an X report rather than a Z. */
	isFinal: boolean;
	currency: string;
	openingFloatMinor: number;
	takingsMinor: number;
	cashFromSalesMinor: number;
	netMovementsMinor: number;
	expectedCashMinor: number;
	/** Absent until the drawer has been counted. */
	countedCashMinor: number | null;
	variance: 'balanced' | 'short' | 'over' | null;
	varianceMinor: number | null;
	saleCount: number;
	voidCount: number;
	countAttempts: number;
}

/** Why cash moved in or out of the drawer outside a sale. */
export type CashReason = 'float_top_up' | 'skim' | 'petty_cash' | 'refund' | 'correction';

/** Live sync state. `disabled` is normal for a shop with no server configured. */
export type SyncView =
	| { state: 'disabled' }
	| { state: 'upToDate'; unsynced: number }
	| { state: 'retrying'; unsynced: number; attempts: number }
	| { state: 'stopped'; reason: string };

export interface TillStatus {
	takingsMinor: number;
	currency: string;
	unsyncedCount: number;
	openSales: number;
}

/** The shape the Rust command layer returns on failure. */
export interface TillError {
	code: string;
	message: string;
}

export class TillUnavailableError extends Error {
	constructor() {
		super('This screen only runs inside the Sahl till application.');
		this.name = 'TillUnavailableError';
	}
}

/** Whether we are running inside the Tauri shell rather than a plain browser tab. */
export function isTillAvailable(): boolean {
	return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

async function call<T>(command: string, args: Record<string, unknown> = {}): Promise<T> {
	if (!isTillAvailable()) {
		throw new TillUnavailableError();
	}
	return invoke<T>(command, args);
}

/** Narrow an unknown thrown value to the till's error shape. */
export function asTillError(error: unknown): TillError {
	if (error instanceof TillUnavailableError) {
		return { code: 'no_till', message: error.message };
	}
	if (typeof error === 'object' && error !== null && 'code' in error && 'message' in error) {
		return error as TillError;
	}
	return { code: 'unknown', message: String(error) };
}

export const till = {
	openSale: (cashierId: string) => call<SaleView>('open_sale', { cashierId }),

	addLine: (input: {
		saleId: string;
		productId: string;
		name: string;
		unitPriceMinor: number;
		quantityMilli: number;
		taxBasisPoints: number;
		currency: string;
	}) => call<SaleView>('add_line', input),

	changeQuantity: (saleId: string, lineId: string, quantityMilli: number) =>
		call<SaleView>('change_quantity', { saleId, lineId, quantityMilli }),

	voidLine: (saleId: string, lineId: string, reason: string, authorizedBy: string) =>
		call<SaleView>('void_line', { saleId, lineId, reason, authorizedBy }),

	recordTender: (input: {
		saleId: string;
		method: string;
		amountMinor: number;
		currency: string;
		reference?: string | null;
	}) => call<SaleView>('record_tender', input),

	completeSale: (saleId: string) => call<SaleView>('complete_sale', { saleId }),

	abandonSale: (saleId: string, abandonedBy: string) =>
		call<SaleView>('abandon_sale', { saleId, abandonedBy }),

	getSale: (saleId: string) => call<SaleView>('get_sale', { saleId }),

	status: () => call<TillStatus>('till_status'),

	syncStatus: () => call<SyncView>('sync_status'),

	openShift: (cashierId: string, openingFloatMinor: number) =>
		call<ShiftView>('open_shift', { cashierId, openingFloatMinor }),

	moveCash: (input: {
		amountMinor: number;
		reason: CashReason;
		note?: string | null;
		authorizedBy: string;
	}) => call<ShiftView>('move_cash', input),

	countDrawer: (countedMinor: number, countedBy: string) =>
		call<ShiftView>('count_drawer', { countedMinor, countedBy }),

	/** The X report — where the shift stands, without ending it. */
	shiftReport: () => call<ShiftView>('shift_report'),

	/**
	 * The same shift with every expectation withheld.
	 *
	 * A separate call rather than a flag, because the safest way to not leak the expected figure is
	 * to never send it to this process.
	 */
	blindCountSheet: () => call<ShiftView>('blind_count_sheet'),

	closeShift: (closedBy: string, closingCashMinor: number) =>
		call<ShiftView>('close_shift', { closedBy, closingCashMinor })
};
