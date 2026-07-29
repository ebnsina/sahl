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

/** One batch as the stock screen shows it. */
export interface BatchView {
	id: string;
	productId: string;
	lot: string | null;
	/** Milliseconds since the epoch. */
	expiresAt: number | null;
	receivedAt: number;
	/** Thousandths of a unit: 1234 is 1.234 kg. Zero on a blind sheet. */
	onHandMilli: number;
	unitCostMinor: number | null;
	negative: boolean;
}

/** A count that disagreed with the book. */
export interface VarianceView {
	batchId: string;
	expectedMilli: number;
	countedMilli: number;
	/** Counted minus expected. Negative means stock is missing. */
	deltaMilli: number;
	at: number;
	countedBy: string;
}

export interface StockView {
	batches: BatchView[];
	variances: VarianceView[];
	currency: string;
}

/** Why stock left a batch outside a sale. */
export type IssueReason = 'wastage' | 'transfer_out' | 'return_to_supplier' | 'internal';

/** One staff member. Never carries a PIN hash — verification happens in Rust. */
export interface StaffView {
	id: string;
	name: string;
	role: 'cashier' | 'manager' | 'owner';
	active: boolean;
}

/** One line of the audit feed, with names already resolved. */
export interface AuditView {
	at: number;
	severity: 'routine' | 'notable' | 'alert';
	kind: string;
	actor: string;
	actorName: string;
	approvedBy: string | null;
	approvedByName: string | null;
	amountMinor: number | null;
	summary: string;
	/** The actor approved their own action and their role did not carry it. */
	unapproved: boolean;
}

/** One order line, with what has arrived against it. */
export interface OrderLineView {
	lineId: string;
	productId: string;
	orderedMilli: number;
	receivedMilli: number;
	/** Ordered minus received. Negative means the supplier sent more than was asked for. */
	outstandingMilli: number;
	unitCostMinor: number;
	receivedValueMinor: number;
	/** The price charged did not match the price ordered. */
	priceChanged: boolean;
}

export interface OrderView {
	id: string;
	supplier: string;
	reference: string | null;
	expectedAt: number | null;
	placedAt: number;
	status: 'awaiting' | 'partly_received' | 'fully_received' | 'closed';
	closeReason: 'complete' | 'short_shipped' | 'cancelled' | 'unknown' | null;
	orderedValueMinor: number;
	receivedValueMinor: number;
	lines: OrderLineView[];
	currency: string;
}

/** Why an order stopped short of being fully received. */
export type CloseReason = 'complete' | 'short_shipped' | 'cancelled';

/** How a supply is treated for VAT. */
export type TaxTreatment = 'standard' | 'zero_rated' | 'exempt';

/** How this outlet trades. */
export interface OutletView {
	outletId: string;
	name: string;
	profile: 'retail' | 'cafe' | 'grocery';
	currency: string;
	timezone: string;
	regime: 'none' | 'bd_mushak';
	taxRegistration: string | null;
	address: string;
	configuredAt: number;
	/** What this profile can do, so a screen need not reimplement the table. */
	capabilities: string[];
}

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
		/** Only read for `standard`. */
		taxBasisPoints: number;
		/**
		 * Three treatments, not one rate. Zero-rated keeps input VAT reclaimable and exempt does
		 * not, so a rate of zero cannot stand in for either.
		 */
		taxTreatment: TaxTreatment;
		quantityMilli: number;
		currency: string;
	}) => call<SaleView>('add_line', input),

	changeQuantity: (saleId: string, lineId: string, quantityMilli: number) =>
		call<SaleView>('change_quantity', { saleId, lineId, quantityMilli }),

	/** `pin` is a manager's own PIN, typed at the till — never an id this app picked. */
	voidLine: (saleId: string, lineId: string, reason: string, pin: string) =>
		call<SaleView>('void_line', { saleId, lineId, reason, pin }),

	recordTender: (input: {
		saleId: string;
		method: string;
		amountMinor: number;
		currency: string;
		reference?: string | null;
	}) => call<SaleView>('record_tender', input),

	completeSale: (saleId: string, cashierId: string) =>
		call<SaleView>('complete_sale', { saleId, cashierId }),

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
		pin: string;
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
		call<ShiftView>('close_shift', { closedBy, closingCashMinor }),

	receiveStock: (input: {
		productId: string;
		lot?: string | null;
		expiresAtMillis?: number | null;
		quantityMilli: number;
		unitCostMinor: number;
		supplier?: string | null;
		receivedBy: string;
	}) => call<StockView>('receive_stock', input),

	countStock: (batchId: string, countedMilli: number, countedBy: string) =>
		call<StockView>('count_stock', { batchId, countedMilli, countedBy }),

	issueStock: (batchId: string, quantityMilli: number, reason: IssueReason, issuedBy: string) =>
		call<StockView>('issue_stock', { batchId, quantityMilli, reason, issuedBy }),

	stockPosition: () => call<StockView>('stock_position'),

	/** The same batches with recorded levels withheld — a blind shelf count. */
	blindStockSheet: () => call<StockView>('blind_stock_sheet'),

	staffList: () => call<StaffView[]>('staff_list'),

	signIn: (staffId: string, pin: string) => call<StaffView>('sign_in', { staffId, pin }),

	enrolStaff: (input: {
		name: string;
		role: StaffView['role'];
		newPin: string;
		/** An owner's PIN. Ignored only when enrolling the very first person. */
		pin: string;
	}) => call<StaffView[]>('enrol_staff', input),

	auditFeed: () => call<AuditView[]>('audit_feed'),

	outletConfig: () => call<OutletView | null>('outlet_config'),

	configureOutlet: (input: {
		name: string;
		profile: OutletView['profile'];
		currency: string;
		timezone: string;
		regime: OutletView['regime'];
		taxRegistration?: string | null;
		address: string;
		/** An owner's PIN. Ignored only before anyone is enrolled. */
		pin: string;
	}) => call<OutletView | null>('configure_outlet', input),

	orderList: () => call<OrderView[]>('order_list'),

	placeOrder: (input: {
		supplier: string;
		reference?: string | null;
		expectedAtMillis?: number | null;
		lines: Array<{ productId: string; quantityMilli: number; unitCostMinor: number }>;
		placedBy: string;
	}) => call<OrderView[]>('place_order', input),

	/** Books the delivery against the order and onto the shelf in one atomic write. */
	receiveAgainstOrder: (input: {
		orderId: string;
		lineId: string;
		quantityMilli: number;
		unitCostMinor: number;
		lot?: string | null;
		expiresAtMillis?: number | null;
		receivedBy: string;
	}) => call<OrderView[]>('receive_against_order', input),

	closeOrder: (orderId: string, reason: CloseReason, closedBy: string) =>
		call<OrderView[]>('close_order', { orderId, reason, closedBy })
};
