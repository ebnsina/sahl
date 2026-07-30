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
	/** Options chosen on this line, so the basket shows what the kitchen was told. */
	modifiers: Array<{ name: string; priceDeltaMinor: number }>;
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
	regime: 'none' | 'bd_mushak' | 'zatca';
	taxRegistration: string | null;
	address: string;
	configuredAt: number;
	/** What this profile can do, so a screen need not reimplement the table. */
	capabilities: string[];
	/** Absent where no scale prints labels, which is every outlet but a grocery. */
	scale: ScaleFormatView | null;
	/** What a cashier may do unaided. All zeros means everything needs somebody else. */
	approval: ApprovalView;
}

/** The thresholds below which a cashier needs nobody else. */
export interface ApprovalView {
	discountLimitMinor: number;
	discountRateBasisPoints: number;
	voidLimitMinor: number;
}

/** How a counter scale lays out the labels it prints. */
export interface ScaleFormatView {
	prefix: string;
	itemDigits: number;
	embedded: 'weight' | 'price';
	valueDigits: number;
	valueDecimals: number;
	fillerDigits: number;
}

/** What a scan resolved to. */
export interface ScanView {
	product: ProductView;
	/** Thousandths. A weighed label brings its own; anything else is one. */
	quantityMilli: number;
	/** Set only where the scale already fixed the money — sell at this, do not reprice. */
	priceMinor: number | null;
}

/** One row of a Mushak 6.3, by the form's own column numbers. */
export interface ChallanLineView {
	serial: number;
	description: string;
	unit: string;
	quantityMilli: number;
	/** Column 5 — unit value, excluding tax. */
	unitValueMinor: number;
	/** Column 6 — total value, excluding tax. */
	totalValueMinor: number;
	supplementaryDutyMinor: number;
	vatRateBasisPoints: number;
	vatAmountMinor: number;
	totalWithTaxMinor: number;
}

/** A fiscal document, or the fact that this outlet owes none. */
export type DocumentView =
	| {
			regime: 'bd_mushak63';
			sellerName: string;
			sellerBin: string;
			issuingAddress: string;
			buyerName: string | null;
			buyerBin: string | null;
			invoiceNumber: string;
			issuedAtMillis: number;
			lines: ChallanLineView[];
			totalValueMinor: number;
			totalVatMinor: number;
			totalWithTaxMinor: number;
	  }
	| {
			regime: 'zatca';
			sellerName: string;
			sellerVat: string;
			issuingAddress: string;
			invoiceNumber: string;
			issuedAtMillis: number;
			lines: ZatcaLineView[];
			totalExcludingVatMinor: number;
			totalVatMinor: number;
			totalWithVatMinor: number;
			/** Base64 TLV. The till decides the payload; a screen only draws the symbol. */
			qr: string;
	  }
	| { regime: 'none' };

/** One line of a ZATCA simplified invoice, stated excluding VAT. */
export interface ZatcaLineView {
	description: string;
	unit: string;
	quantityMilli: number;
	unitPriceMinor: number;
	lineTotalMinor: number;
	vatRateBasisPoints: number;
	vatAmountMinor: number;
	totalWithVatMinor: number;
}

/** One thing the log says worth an owner's attention. */
export interface FindingView {
	kind: string;
	severity: 'routine' | 'notable' | 'alert';
	/** The person's name, or absent where it is about the outlet. */
	person: string | null;
	count: number;
	amountMinor: number | null;
	/** States what was counted. Never what it implies. */
	summary: string;
}

/** What happened when a receipt was sent to a printer. */
export interface PrintOutcome {
	printed: boolean;
	/** Why not, when it did not. Never a reason to undo a sale. */
	reason: string | null;
	bytes: number;
}

/** One choice within a group — "Large", "Oat milk". */
export interface ModifierOption {
	id: string;
	name: string;
	/** What choosing it adds to **one unit**. Zero and negative are both real. */
	priceDeltaMinor: number;
}

/**
 * A set of choices offered on a product.
 *
 * Grouped rather than flat because the two shapes behave differently: "size" is exactly one of
 * small/medium/large, "extras" is any number. A flat list lets a cashier pick small *and* large.
 */
export interface ModifierGroup {
	id: string;
	name: string;
	/** Fewest choices that must be made. One means it cannot be skipped. */
	min: number;
	/** Most that may be made. One makes it a single choice. */
	max: number;
	options: ModifierOption[];
}

/** A product as the sell screen and the catalogue screen show it. */
export interface ProductView {
	id: string;
	name: string;
	sku: string | null;
	barcodes: string[];
	priceMinor: number;
	/** `pcs`, `kg`, `L` — printed on the receipt and in the Mushak Unit of Supply column. */
	unit: string;
	/** Whether the unit sells in fractions. Selling 0.4 of a piece is a mis-key. */
	divisible: boolean;
	taxBasisPoints: number;
	taxTreatment: TaxTreatment;
	category: string | null;
	active: boolean;
	/** Where this is made, for a café. */
	station: string | null;
	/** Choices offered when this is rung, so the sell screen can draw the chooser. */
	optionGroups: ModifierGroup[];
}

/** A table as the floor plan shows it. */
export interface TableView {
	id: string;
	label: string;
	section: string | null;
	seats: number;
	active: boolean;
	/** The open ticket sitting here, if any. Derived from the sales, never stored on the table. */
	saleId: string | null;
	runningTotalMinor: number | null;
	covers: number | null;
}

/** An open ticket, as the ticket list shows it. */
export interface TicketView {
	saleId: string;
	lineCount: number;
	/** `null` for a ticket with nothing on it yet. */
	totalMinor: number | null;
	tableLabel: string | null;
	covers: number | null;
	/** Another device is holding it; it cannot be written to from here. */
	heldElsewhere: boolean;
}

/** One person's share of a split bill. */
export interface SplitPartView {
	number: number;
	amountMinor: number;
	/** The lines this part covers. Empty for an even split. */
	lineIds: string[];
}

/** One station's instruction. */
export interface KitchenTicketView {
	station: string;
	/** `order` or `cancellation` — never conflated. */
	kind: 'order' | 'cancellation';
	tableLabel: string | null;
	covers: number | null;
	round: number;
	lines: Array<{ name: string; quantityMilli: number; modifiers: string[] }>;
}

export interface FireOutcome {
	tickets: KitchenTicketView[];
	printed: boolean;
	/** Why not. The order is recorded either way. */
	reason: string | null;
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
	/** No actor: the till uses whoever is signed in. An id the screen chose records nothing. */
	openSale: () => call<SaleView>('open_sale'),

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
		/** Option ids chosen at the till. The till validates them against the product's groups. */
		chosenOptions: string[];
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

	completeSale: (saleId: string) => call<SaleView>('complete_sale', { saleId }),

	abandonSale: (saleId: string) => call<SaleView>('abandon_sale', { saleId }),

	getSale: (saleId: string) => call<SaleView>('get_sale', { saleId }),

	/** Every ticket still open on this outlet. Without this they are unreachable. */
	openTickets: () => call<TicketView[]>('open_tickets'),

	/**
	 * Work out each share of a bill. Records nothing — a split is arithmetic, and the shares are
	 * then taken through the ordinary tender path.
	 *
	 * Pass an empty `lineAssignment` to split evenly `ways` times.
	 */
	splitBill: (saleId: string, ways: number, lineAssignment: string[][] = []) =>
		call<SplitPartView[]>('split_bill', { saleId, ways, lineAssignment }),

	/** What each station has not yet been told about. */
	pendingKitchen: (saleId: string) => call<KitchenTicketView[]>('pending_kitchen', { saleId }),

	/**
	 * Send everything new to its station.
	 *
	 * Records the firing before printing and never undoes it on a print failure — a rollback would
	 * let the next press resend lines a station may already have on a half-printed slip.
	 */
	fireKitchen: (input: { saleId: string; printedAt: string; paper: 'mm58' | 'mm80' }) =>
		call<FireOutcome>('fire_kitchen', input),

	/** Abandon tickets with nothing on them. Never touches one holding items. */
	discardEmptyTickets: () => call<number>('discard_empty_tickets'),

	status: () => call<TillStatus>('till_status'),

	syncStatus: () => call<SyncView>('sync_status'),

	openShift: (openingFloatMinor: number) => call<ShiftView>('open_shift', { openingFloatMinor }),

	moveCash: (input: {
		amountMinor: number;
		reason: CashReason;
		note?: string | null;
		pin: string;
	}) => call<ShiftView>('move_cash', input),

	countDrawer: (countedMinor: number) => call<ShiftView>('count_drawer', { countedMinor }),

	/** The X report — where the shift stands, without ending it. */
	shiftReport: () => call<ShiftView>('shift_report'),

	/**
	 * The same shift with every expectation withheld.
	 *
	 * A separate call rather than a flag, because the safest way to not leak the expected figure is
	 * to never send it to this process.
	 */
	blindCountSheet: () => call<ShiftView>('blind_count_sheet'),

	closeShift: (closingCashMinor: number) => call<ShiftView>('close_shift', { closingCashMinor }),

	receiveStock: (input: {
		productId: string;
		lot?: string | null;
		expiresAtMillis?: number | null;
		quantityMilli: number;
		unitCostMinor: number;
		supplier?: string | null;
	}) => call<StockView>('receive_stock', input),

	countStock: (batchId: string, countedMilli: number) =>
		call<StockView>('count_stock', { batchId, countedMilli }),

	issueStock: (batchId: string, quantityMilli: number, reason: IssueReason) =>
		call<StockView>('issue_stock', { batchId, quantityMilli, reason }),

	stockPosition: () => call<StockView>('stock_position'),

	/** The same batches with recorded levels withheld — a blind shelf count. */
	blindStockSheet: () => call<StockView>('blind_stock_sheet'),

	staffList: () => call<StaffView[]>('staff_list'),

	signIn: (staffId: string, pin: string) => call<StaffView>('sign_in', { staffId, pin }),

	/**
	 * Who is at the till, or nobody. Asked rather than cached — a session expires by being read,
	 * so a held copy would keep selling as somebody who walked away.
	 */
	currentSession: () => call<StaffView | null>('current_session'),

	signOut: () => call<void>('sign_out'),

	/** Whether this build can seed demo data at all. False in a release binary. */
	canSeed: () => call<boolean>('can_seed'),

	/** Fill an empty till with a demo shop. Returns the PIN every demo account shares. */
	seedDemo: (market: 'bangladesh' | 'gulf') => call<string>('seed_demo', { market }),

	enrolStaff: (input: {
		name: string;
		role: StaffView['role'];
		newPin: string;
		/** An owner's PIN. Ignored only when enrolling the very first person. */
		pin: string;
	}) => call<StaffView[]>('enrol_staff', input),

	auditFeed: () => call<AuditView[]>('audit_feed'),

	/** What the log says about how the till is being used. Questions, not accusations. */
	anomalyFeed: () => call<FindingView[]>('anomaly_feed'),

	outletConfig: () => call<OutletView | null>('outlet_config'),

	/** Rebuilt from the log on demand — never stored, so it cannot disagree with the sale. */
	fiscalDocument: (saleId: string) => call<DocumentView>('fiscal_document', { saleId }),

	printerConfigured: () => call<boolean>('printer_configured'),

	sellableProducts: () => call<ProductView[]>('sellable_products'),

	floorPlan: (includeRemoved = false) => call<TableView[]>('floor_plan', { includeRemoved }),

	saveTable: (input: {
		/** Absent for a new table. */
		tableId?: string | null;
		label: string;
		section?: string | null;
		seats: number;
		pin: string;
	}) => call<TableView[]>('save_table', input),

	setTableActive: (tableId: string, active: boolean, pin: string) =>
		call<TableView[]>('set_table_active', { tableId, active, pin }),

	/** Seat a ticket, or move it to another table. */
	seatSale: (saleId: string, tableId: string, covers: number) =>
		call<SaleView>('seat_sale', { saleId, tableId, covers }),

	allProducts: () => call<ProductView[]>('all_products'),

	/** `null` for an unrecognised code — an ordinary event at a counter, not a fault. */
	scan: (barcode: string) => call<ScanView | null>('scan', { barcode }),

	saveProduct: (input: {
		/** Absent for a new product. */
		productId?: string | null;
		name: string;
		sku?: string | null;
		barcodes: string[];
		priceMinor: number;
		unit: string;
		taxBasisPoints: number;
		taxTreatment: TaxTreatment;
		category?: string | null;
		station?: string | null;
		/** Ids are absent for anything newly added; the till mints them. */
		optionGroups: Array<{
			id: string | null;
			name: string;
			min: number;
			max: number;
			options: Array<{ id: string | null; name: string; priceDeltaMinor: number }>;
		}>;
		pin: string;
	}) => call<ProductView[]>('save_product', input),

	setProductActive: (productId: string, active: boolean, pin: string) =>
		call<ProductView[]>('set_product_active', { productId, active, pin }),

	printReceipt: (input: {
		saleId: string;
		/** Pre-formatted with `Intl` in the outlet's timezone — only the caller knows the outlet. */
		printedAt: string;
		paper: 'mm58' | 'mm80';
		openDrawer: boolean;
	}) => call<PrintOutcome>('print_receipt', input),

	configureOutlet: (input: {
		name: string;
		profile: OutletView['profile'];
		currency: string;
		timezone: string;
		regime: OutletView['regime'];
		taxRegistration?: string | null;
		address: string;
		scale?: ScaleFormatView | null;
		approval?: ApprovalView | null;
		/** An owner's PIN. Ignored only before anyone is enrolled. */
		pin: string;
	}) => call<OutletView | null>('configure_outlet', input),

	orderList: () => call<OrderView[]>('order_list'),

	placeOrder: (input: {
		supplier: string;
		reference?: string | null;
		expectedAtMillis?: number | null;
		lines: Array<{ productId: string; quantityMilli: number; unitCostMinor: number }>;
	}) => call<OrderView[]>('place_order', input),

	/** Books the delivery against the order and onto the shelf in one atomic write. */
	receiveAgainstOrder: (input: {
		orderId: string;
		lineId: string;
		quantityMilli: number;
		unitCostMinor: number;
		lot?: string | null;
		expiresAtMillis?: number | null;
	}) => call<OrderView[]>('receive_against_order', input),

	closeOrder: (orderId: string, reason: CloseReason) =>
		call<OrderView[]>('close_order', { orderId, reason })
};
