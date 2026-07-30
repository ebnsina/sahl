<script lang="ts">
	/**
	 * The sell screen.
	 *
	 * Runs in `touch` density throughout: 44px minimum targets, larger type, taller rows. A cashier
	 * works this screen fast, sometimes on a resistive panel, sometimes without looking — mis-taps
	 * here cost money and trust, so the density is not a preference.
	 *
	 * Every amount rendered arrives as an exact integer from Rust and is formatted by `Intl`. There
	 * is no arithmetic in this file, and there must never be.
	 */
	import Ban from '@lucide/svelte/icons/ban';
	import Printer from '@lucide/svelte/icons/printer';
	import ReceiptText from '@lucide/svelte/icons/receipt-text';
	import Banknote from '@lucide/svelte/icons/banknote';
	import Check from '@lucide/svelte/icons/check';
	import Plus from '@lucide/svelte/icons/plus';
	import Trash2 from '@lucide/svelte/icons/trash-2';
	import { Badge, Button, Card, Field, Input, Numeric, Logo, parseMinor } from '@sahl/ui';
	import PinPrompt from '$lib/PinPrompt.svelte';
	import SignIn from '$lib/SignIn.svelte';
	import { loadShop, shop } from '$lib/outlet.svelte';
	import {
		asTillError,
		isTillAvailable,
		till,
		type SaleView,
		type DocumentView,
		type PrintOutcome,
		type TicketView,
		type SyncView,
		type ModifierGroup,
		type ProductView,
		type KitchenTicketView,
		type SplitPartView,
		type StaffView,
		type TillStatus
	} from '$lib/till';

	/** The real catalogue, from the till. Empty until someone adds a product. */
	let catalogue = $state<ProductView[]>([]);

	// The outlet's own currency and timezone, not this screen's guess.
	const format = $derived(shop.formatters);

	let sale = $state<SaleView | null>(null);
	/** The challan for the sale just completed, if this outlet issues one. */
	let document = $state<DocumentView | null>(null);
	/** Why a challan could not be issued. Surfaced, never allowed to block the sale. */
	let documentProblem = $state<string | null>(null);
	let hasPrinter = $state(false);
	/** Who is at the till. Re-asked rather than cached — the session expires by being read. */
	let who = $state<StaffView | null>(null);
	/** Open tickets, so one a cashier navigated away from is reachable again. */
	let tickets = $state<TicketView[]>([]);
	let showTickets = $state(false);
	/** The product whose options are being chosen, if any. */
	let choosing = $state<ProductView | null>(null);
	/** Option ids picked so far, across every group. */
	let chosen = $state<string[]>([]);
	/** Shares of the bill, once someone asks to split it. */
	let split = $state<SplitPartView[] | null>(null);
	let splitWays = $state(2);
	/** The last barcode nothing matched, so the cashier sees which scan was ignored. */
	let scanMiss = $state<string | null>(null);
	/** What the stations have not yet been told about this ticket. */
	let pendingTickets = $state<KitchenTicketView[]>([]);
	let fireOutcome = $state<{ printed: boolean; reason: string | null } | null>(null);
	let printOutcome = $state<PrintOutcome | null>(null);
	let status = $state<TillStatus | null>(null);
	let sync = $state<SyncView | null>(null);
	let error = $state<{ code: string; message: string } | null>(null);
	let busy = $state(false);
	let cashInput = $state('');
	let available = $state(true);
	/** Set while a void is waiting on a manager's PIN. */
	let pendingVoid = $state<string | null>(null);
	let approvalError = $state<string | null>(null);

	// Read straight off the till's own numbers — never recomputed here.
	let settled = $derived(sale?.status === 'completed');
	let balanceDue = $derived(sale?.balanceDueMinor ?? 0);

	async function run<T>(action: () => Promise<T>, onDone?: (result: T) => void) {
		busy = true;
		error = null;
		try {
			const result = await action();
			onDone?.(result);
			status = await till.status();
			sync = await till.syncStatus();
			tickets = await till.openTickets();
			await refreshKitchen(sale?.id);
		} catch (thrown) {
			error = asTillError(thrown);
			if (error.code === 'no_till') available = false;
			// The session went idle mid-transaction. Back to the sign-in rather than leaving a
			// screen that looks live and refuses everything.
			if (error.code === 'not_signed_in') {
				who = null;
				sale = null;
			}
		} finally {
			busy = false;
		}
	}

	$effect(() => {
		available = isTillAvailable();
		if (available) {
			void loadShop();
			// Asked, never assumed: a till that was asleep reports nobody the moment it is read.
			void till.currentSession().then((member) => (who = member));
			void run(
				() => till.status(),
				(result) => (status = result)
			);
			void till.printerConfigured().then((configured) => (hasPrinter = configured));
			void run(
				() => till.sellableProducts(),
				(result) => (catalogue = result)
			);
			void refreshTickets();
		}
	});

	async function refreshKitchen(saleId: string | undefined) {
		if (!saleId) {
			pendingTickets = [];
			return;
		}
		try {
			pendingTickets = await till.pendingKitchen(saleId);
		} catch {
			// A kitchen view that cannot load must not stop someone selling.
			pendingTickets = [];
		}
	}

	function fireKitchen() {
		const current = sale;
		if (!current) return;
		void run(
			() =>
				till.fireKitchen({
					saleId: current.id,
					printedAt: format.dateTime(Date.now()),
					paper: 'mm80'
				}),
			(result) => {
				fireOutcome = { printed: result.printed, reason: result.reason };
				void refreshKitchen(current.id);
			}
		);
	}

	async function refreshTickets() {
		try {
			tickets = await till.openTickets();
			await refreshKitchen(sale?.id);
		} catch {
			// A ticket list that cannot load must not stop someone selling.
			tickets = [];
		}
	}

	function resumeTicket(saleId: string) {
		void run(
			() => till.getSale(saleId),
			(result) => {
				sale = result;
				showTickets = false;
				cashInput = '';
				document = null;
				documentProblem = null;
			}
		);
	}

	function discardEmpty() {
		void run(
			() => till.discardEmptyTickets(),
			() => void refreshTickets()
		);
	}

	function startSale() {
		document = null;
		documentProblem = null;
		printOutcome = null;
		split = null;
		fireOutcome = null;
		pendingTickets = [];
		void run(
			() => till.openSale(),
			(result) => {
				sale = result;
				cashInput = '';
			}
		);
	}

	function addItem(item: ProductView) {
		const current = sale;
		if (!current) return;

		// A product that offers choices cannot be one-tapped: the till refuses a line whose required
		// groups are unsatisfied, so asking here is the difference between a chooser and an error.
		if (item.optionGroups.length > 0) {
			chosen = [];
			choosing = item;
			return;
		}
		ring(item, []);
	}

	/** Whether every required group has been satisfied. Mirrors what the till enforces. */
	function choicesComplete(product: ProductView, picked: string[]): boolean {
		return product.optionGroups.every((group) => {
			const count = group.options.filter((option) => picked.includes(option.id)).length;
			return count >= group.min && count <= group.max;
		});
	}

	function toggleChoice(group: ModifierGroup, optionId: string) {
		if (chosen.includes(optionId)) {
			chosen = chosen.filter((id) => id !== optionId);
			return;
		}
		if (group.max === 1) {
			// Single choice: picking one replaces the other rather than erroring afterwards.
			const others = group.options.map((option) => option.id);
			chosen = [...chosen.filter((id) => !others.includes(id)), optionId];
			return;
		}
		chosen = [...chosen, optionId];
	}

	function confirmChoices() {
		const product = choosing;
		if (!product) return;
		ring(product, chosen);
		choosing = null;
	}

	function ring(
		item: ProductView,
		chosenOptions: string[],
		quantityMilli = 1000,
		priceMinor = item.priceMinor
	) {
		const current = sale;
		if (!current) return;
		void run(
			() =>
				till.addLine({
					saleId: current.id,
					productId: item.id,
					name: item.name,
					unitPriceMinor: priceMinor,
					// One unit per tap. A divisible product still needs a real quantity, which is what
					// a scale label or a keypad supplies — tapping it is not a weighing.
					quantityMilli,
					taxBasisPoints: item.taxBasisPoints,
					taxTreatment: item.taxTreatment,
					chosenOptions
				}),
			(result) => (sale = result)
		);
	}

	function scan(event: KeyboardEvent) {
		// A hardware scanner types the digits and presses Enter, which is why nothing here listens
		// for individual keystrokes or races a timer.
		if (event.key !== 'Enter') return;
		const field = event.target as HTMLInputElement;
		const barcode = field.value.trim();
		if (!barcode || !sale) return;
		field.value = '';
		scanMiss = null;

		void run(
			() => till.scan(barcode),
			(result) => {
				if (!result) {
					// Not a fault. A loyalty card, a coupon, a competitor's packaging.
					scanMiss = barcode;
					return;
				}
				// A scale label already decided the weight, and sometimes the money too — passing
				// the catalogue price back would disagree with the sticker in the customer's hand.
				ring(
					result.product,
					[],
					result.quantityMilli,
					result.priceMinor ?? result.product.priceMinor
				);
			}
		);
	}

	function signOut() {
		void (async () => {
			await till.signOut();
			who = null;
			sale = null;
		})();
	}

	function voidLine(lineId: string) {
		// The approval is not this screen's to grant. It asks, the till decides.
		approvalError = null;
		pendingVoid = lineId;
	}

	function confirmVoid(pin: string) {
		const current = sale;
		const lineId = pendingVoid;
		if (!current || !lineId) return;
		void (async () => {
			busy = true;
			approvalError = null;
			try {
				sale = await till.voidLine(current.id, lineId, 'mistake', pin);
				pendingVoid = null;
			} catch (thrown) {
				const failure = asTillError(thrown);
				// A refused PIN keeps the prompt open — the manager mistyped and will try again.
				// Anything else is a real fault and belongs in the page-level error strip.
				if (failure.code === 'not_authorized' || failure.code === 'no_approver') {
					approvalError = failure.message;
				} else {
					error = failure;
					pendingVoid = null;
				}
			} finally {
				busy = false;
			}
		})();
	}

	function splitEvenly(ways: number) {
		const current = sale;
		if (!current) return;
		splitWays = ways;
		void run(
			() => till.splitBill(current.id, ways),
			(result) => (split = result)
		);
	}

	/**
	 * Take one share as cash.
	 *
	 * The share is just a tender — a split is arithmetic, and the sale has recorded partial tenders
	 * since P1. Nothing about the sale knows it was split.
	 */
	function tenderShare(amountMinor: number) {
		const current = sale;
		if (!current) return;
		void run(
			() =>
				till.recordTender({
					saleId: current.id,
					method: 'cash',
					amountMinor
				}),
			(result) => (sale = result)
		);
	}

	function tenderCash() {
		const current = sale;
		if (!current) return;
		const amountMinor = parseMinor(cashInput, shop.currency ?? 'BDT');
		if (amountMinor === null || amountMinor <= 0) {
			error = { code: 'bad_amount', message: 'Enter a cash amount like 500 or 499.50' };
			return;
		}
		void run(
			() => till.recordTender({ saleId: current.id, method: 'cash', amountMinor }),
			(result) => {
				sale = result;
				cashInput = '';
			}
		);
	}

	function tenderExact() {
		const current = sale;
		if (!current) return;
		void run(
			() =>
				till.recordTender({
					saleId: current.id,
					method: 'cash',
					amountMinor: current.balanceDueMinor
				}),
			(result) => (sale = result)
		);
	}

	function complete() {
		const current = sale;
		if (!current) return;
		void run(
			() => till.completeSale(current.id),
			(result) => {
				sale = result;
				// Fetched after the sale is already settled, and its failure never undoes it. A till
				// that refused to sell because a document could not be issued would be a till a
				// shopkeeper stops using.
				void fetchDocument(result.id);
			}
		);
	}

	function printReceipt(openDrawer: boolean) {
		const current = sale;
		if (!current) return;
		void run(
			() =>
				till.printReceipt({
					saleId: current.id,
					// Only this side knows the outlet's timezone, so the receipt's date is formatted
					// here rather than in Rust — the same reason the renderer refuses to format it.
					printedAt: format.dateTime(Date.now()),
					paper: 'mm80',
					openDrawer
				}),
			(result) => (printOutcome = result)
		);
	}

	async function fetchDocument(saleId: string) {
		document = null;
		documentProblem = null;
		try {
			document = await till.fiscalDocument(saleId);
		} catch (thrown) {
			documentProblem = asTillError(thrown).message;
		}
	}

	function abandon() {
		const current = sale;
		if (!current) return;
		void run(
			() => till.abandonSale(current.id),
			() => (sale = null)
		);
	}
</script>

<svelte:head><title>Sell · Sahl</title></svelte:head>

<div data-density="touch" class="bg-canvas text-text flex h-dvh flex-col">
	<header class="border-border bg-surface flex shrink-0 items-center gap-3 border-b px-4 py-2.5">
		<Logo size={22} withWordmark />

		<div class="ms-auto flex items-center gap-2">
			{#if !available}
				<!-- The designed degraded state, not a stack trace. A cashier never sees a crash. -->
				<Badge tone="danger" dot>Till not connected</Badge>
			{:else if status}
				{#if sync?.state === 'stopped'}
					<!-- Distinct from retrying on purpose: this one needs a person, not patience. -->
					<Badge tone="danger" dot>Sync stopped — call support</Badge>
				{:else if sync?.state === 'retrying'}
					<Badge tone="offline" dot>
						Offline · {format.integer(sync.unsynced)} waiting
					</Badge>
				{:else if status.unsyncedCount > 0}
					<Badge tone="unsynced" dot>{format.integer(status.unsyncedCount)} unsynced</Badge>
				{:else if sync?.state === 'disabled'}
					<!-- No server configured. A single-till shop is a valid deployment, not a fault. -->
					<Badge tone="neutral">Local only</Badge>
				{:else}
					<Badge tone="success" dot>Synced</Badge>
				{/if}
				{#if tickets.length > 0}
					<Button
						variant="ghost"
						size="xs"
						icon={ReceiptText}
						onclick={() => (showTickets = !showTickets)}
					>
						{format.integer(tickets.length)} open
					</Button>
				{/if}
				<span class="label-caps">Takings</span>
				<Numeric value={format.money(status.takingsMinor)} class="font-semibold" />
			{/if}
			<a href="/today" class="text-secondary text-text-secondary hover:text-text underline">
				Today
			</a>
			<a href="/shift" class="text-secondary text-text-secondary hover:text-text underline">
				Shift
			</a>
			<a href="/stock" class="text-secondary text-text-secondary hover:text-text underline">
				Stock
			</a>
			<a href="/floor" class="text-secondary text-text-secondary hover:text-text underline">
				Floor
			</a>
			<a href="/catalogue" class="text-secondary text-text-secondary hover:text-text underline">
				Catalogue
			</a>
			<a href="/staff" class="text-secondary text-text-secondary hover:text-text underline">
				Staff
			</a>
			<a href="/orders" class="text-secondary text-text-secondary hover:text-text underline">
				Orders
			</a>
			<a href="/settings" class="text-secondary text-text-secondary hover:text-text underline">
				Settings
			</a>
			{#if who}
				<span class="text-secondary text-text-secondary">{who.name}</span>
				<button
					type="button"
					class="text-secondary text-text-secondary hover:text-text underline"
					onclick={signOut}
				>
					Sign out
				</button>
			{/if}
		</div>
	</header>

	{#if !available}
		<div class="flex flex-1 items-center justify-center p-8">
			<Card label="Not connected" class="max-w-lg">
				<div class="flex flex-col gap-3">
					<p class="text-md">This screen runs inside the Sahl till application.</p>
					<p class="text-secondary text-text-secondary">
						Opening it in a browser shows no data on purpose. The till owns every calculation, and a
						browser stand-in would mean a second implementation of the money rules — the exact drift
						the design exists to prevent.
					</p>
					<p class="text-secondary text-text-muted">
						Run <code class="numeric">cargo tauri dev</code> to start the till.
					</p>
				</div>
			</Card>
		</div>
	{:else if !who}
		<!-- Nothing rings until the till knows whose sale it is. Every per-cashier figure and every
		     question the anomaly feed asks is built on that id. -->
		<SignIn onsignedin={(member) => (who = member)} />
	{:else}
		<div class="grid flex-1 grid-cols-1 gap-4 overflow-hidden p-4 lg:grid-cols-[1fr_26rem]">
			<Card label="Items" class="flex min-h-0 flex-col">
				<div class="mb-2 flex flex-col gap-1">
					<!-- A hardware scanner types and presses Enter, so this is an ordinary field that
					     clears itself. Nothing about a scale label looks different from a supplier one. -->
					<Input
						id="scan"
						placeholder="Scan a barcode"
						numeric
						forceLtr
						disabled={!sale || busy}
						onkeydown={scan}
					/>
					{#if scanMiss}
						<p class="text-secondary text-text-muted">
							Nothing matched <span class="numeric">{scanMiss}</span>.
						</p>
					{/if}
				</div>

				{#if catalogue.length === 0}
					<div class="flex flex-col items-start gap-2 p-2">
						<p class="text-secondary text-text-muted">
							Nothing to sell yet. Add products to the catalogue and they appear here.
						</p>
						<a href="/catalogue" class="text-secondary text-primary-text underline">
							Open the catalogue
						</a>
					</div>
				{/if}
				<div class="grid grid-cols-2 gap-3 sm:grid-cols-3">
					{#each catalogue as item (item.id)}
						<button
							type="button"
							disabled={!sale || settled || busy}
							onclick={() => addItem(item)}
							class="border-border bg-surface hover:bg-surface-hover flex flex-col items-start gap-1 border p-3
							       text-start transition-colors disabled:cursor-not-allowed
							       disabled:opacity-50"
							style="min-height: var(--scale-touch-target)"
						>
							<span class="text-body font-medium">{item.name}</span>
							<Numeric value={format.money(item.priceMinor)} align="start" class="text-secondary" />
							<div class="flex flex-wrap items-center gap-1">
								{#if item.taxTreatment === 'exempt'}
									<Badge tone="neutral">Exempt</Badge>
								{:else if item.taxTreatment === 'zero_rated'}
									<Badge tone="neutral">Zero-rated</Badge>
								{:else}
									<Badge tone="neutral">{format.percent(item.taxBasisPoints)} VAT</Badge>
								{/if}
								{#if item.unit !== 'pcs'}
									<Badge tone="neutral">per {item.unit}</Badge>
								{/if}
								{#if item.optionGroups.length > 0}
									<Badge tone="primary">Options</Badge>
								{/if}
							</div>
						</button>
					{/each}
				</div>
			</Card>

			<div class="flex min-h-0 flex-col gap-3 overflow-y-auto">
				{#if showTickets}
					<Card label="Open tickets" flush>
						<div class="max-h-64 overflow-y-auto">
							{#each tickets as ticket (ticket.saleId)}
								<button
									type="button"
									disabled={busy || ticket.heldElsewhere}
									onclick={() => resumeTicket(ticket.saleId)}
									class="border-border hover:bg-surface-hover flex w-full items-center gap-3 border-b
									       px-3 text-start disabled:cursor-not-allowed disabled:opacity-50"
									style="min-height: var(--scale-row-height)"
								>
									<div class="min-w-0 flex-1">
										<p class="text-body truncate">
											{#if ticket.tableLabel}
												Table {ticket.tableLabel}
											{:else if ticket.lineCount === 0}
												Empty ticket
											{:else}
												{format.integer(ticket.lineCount)} items
											{/if}
										</p>
										{#if ticket.covers !== null}
											<p class="text-secondary text-text-muted">
												{format.integer(ticket.covers)} covers
											</p>
										{/if}
									</div>
									{#if ticket.heldElsewhere}
										<Badge tone="offline" dot>On another till</Badge>
									{/if}
									{#if ticket.totalMinor !== null}
										<Numeric value={format.moneyPlain(ticket.totalMinor)} />
									{/if}
								</button>
							{/each}
						</div>

						{#if tickets.some((ticket) => ticket.lineCount === 0)}
							<div class="border-border border-t p-3">
								<!-- Empty tickets are debris: nobody rang anything, so there is nothing to audit.
								     Tickets holding items are never cleared this way — an abandoned basket full
								     of scanned goods is itself a signal an owner should see. -->
								<Button variant="ghost" size="sm" onclick={discardEmpty} disabled={busy}>
									Clear the empty ones
								</Button>
							</div>
						{/if}
					</Card>
				{/if}

				<!-- A floor, not just a share. `flex-1` alone let the payment controls squeeze the
				     lines to nothing — the cart scrolled out of sight entirely, which is the one thing
				     on this screen a cashier is always reading. -->
				<Card
					label="Sale"
					flush
					class="flex min-h-[16rem] flex-1 flex-col"
					bodyClass="flex min-h-0 flex-1 flex-col overflow-hidden"
				>
					<div class="flex min-h-0 flex-1 flex-col">
						{#if !sale}
							<div class="flex flex-1 flex-col items-center justify-center gap-3 p-6">
								<p class="text-secondary text-text-muted">No sale in progress.</p>
								<Button variant="primary" size="lg" icon={Plus} onclick={startSale} disabled={busy}>
									Start a sale
								</Button>
							</div>
						{:else}
							<div class="min-h-0 flex-1 overflow-y-auto">
								{#if sale.lines.length === 0}
									<p class="text-secondary text-text-muted p-4">Tap an item to begin.</p>
								{/if}
								{#each sale.lines as line (line.id)}
									<div
										class="border-border flex items-center gap-3 border-b px-3"
										style="min-height: var(--scale-row-height)"
									>
										<div class="min-w-0 flex-1">
											<p
												class="text-body truncate {line.voided
													? 'text-text-muted line-through'
													: ''}"
											>
												{line.name}
											</p>
											<p class="text-secondary text-text-muted">
												{format.quantity(line.quantityMilli)} × {format.moneyPlain(
													line.unitPriceMinor
												)}
											</p>
										</div>

										{#if line.voided}
											<Badge tone="voided">Void</Badge>
										{:else}
											<Numeric value={format.moneyPlain(line.totalMinor)} />
											{#if !settled}
												<Button
													variant="ghost"
													size="xs"
													icon={Trash2}
													onclick={() => voidLine(line.id)}
													disabled={busy}>Void</Button
												>
											{/if}
										{/if}
									</div>
								{/each}
							</div>

							{#if pendingTickets.length > 0}
								<!-- Beside the order rather than inside Payment: firing a course is something
								     you do to the order, and it was crushing the lines off the screen from
								     there. A summary, not a second copy of the cart — the lines are directly
								     above, and repeating them is how the panel got too tall to read. -->
								<div
									class="border-primary bg-primary-subtle flex shrink-0 flex-wrap items-center
									       gap-x-3 gap-y-2 border-t p-3"
								>
									<div class="min-w-0 flex-1">
										<div class="flex items-baseline gap-2">
											<span class="label-caps">Not sent yet</span>
											<span class="text-secondary text-text-secondary">
												Round {format.integer(pendingTickets[0].round)}
											</span>
										</div>
										<p class="text-secondary text-text-secondary mt-0.5">
											{pendingTickets
												.map(
													(ticket) =>
														`${ticket.station}${ticket.kind === 'cancellation' ? ' (cancel)' : ''} ${ticket.lines.length}`
												)
												.join(' · ')}
										</p>
									</div>

									<Button variant="primary" onclick={fireKitchen} disabled={busy}>
										Send to kitchen
									</Button>
								</div>
							{:else if fireOutcome && !fireOutcome.printed}
								<!-- The order is recorded either way. Rolling it back on a print failure would
								     let the next press resend lines a station may already have. -->
								<div class="border-warn bg-warn-subtle text-warn-text shrink-0 border-t p-3">
									<p class="label-caps">Sent, but not printed</p>
									<p class="text-secondary mt-1">{fireOutcome.reason}</p>
									<p class="text-secondary mt-1">
										The order is recorded — tell the station directly rather than sending again.
									</p>
								</div>
							{/if}

							<div class="border-border bg-surface-sunken shrink-0 border-t p-3">
								<!-- One grid, not one per row. Separate grids size their `auto` columns
								     independently, so the amounts land at four different x-positions — which
								     defeats the tabular figures the whole numeric style exists for.
								     Two headed columns because a single column mixing tax charged with the
								     value of an exempt supply is a block a reader can add up and get nonsense
								     from. Same split as Mushak columns 6 and 9. -->
								<div
									class="grid items-baseline gap-x-3 gap-y-1"
									style="grid-template-columns: 1fr auto auto"
								>
									<span class="label-caps"></span>
									<span class="label-caps text-end">Taxable</span>
									<span class="label-caps text-end">VAT</span>

									{#each sale.taxGroups as group (group.class + group.basisPoints)}
										<span class="text-secondary text-text-secondary">
											{group.class === 'exempt'
												? 'Exempt'
												: group.class === 'zero_rated'
													? 'Zero-rated'
													: `VAT ${format.percent(group.basisPoints)}`}
										</span>
										<Numeric
											value={format.moneyPlain(group.taxableBaseMinor)}
											class="text-secondary"
										/>
										{#if group.class === 'standard'}
											<Numeric value={format.moneyPlain(group.taxMinor)} class="text-secondary" />
										{:else}
											<!-- A dash, not a zero. Nothing was charged, and a zero in a money column
											     reads as an amount someone calculated. -->
											<span class="numeric text-text-muted block text-end">—</span>
										{/if}
									{/each}
								</div>

								<div class="mt-1 flex flex-col gap-1">
									<div class="border-border flex items-baseline justify-between border-t pt-2">
										<span class="text-md font-semibold">Total</span>
										<Numeric value={format.money(sale.totalMinor)} class="text-lg font-semibold" />
									</div>

									{#if sale.tenderedMinor > 0}
										<div class="text-secondary text-text-secondary flex justify-between">
											<span>Tendered</span>
											<Numeric
												value={format.moneyPlain(sale.tenderedMinor)}
												class="text-secondary"
											/>
										</div>
									{/if}

									{#if balanceDue > 0}
										<div class="flex items-baseline justify-between">
											<span class="text-body text-warn-text font-medium">Balance due</span>
											<Numeric
												value={format.money(balanceDue)}
												class="text-warn-text font-semibold"
											/>
										</div>
									{:else if sale.changeDueMinor > 0}
										<div class="flex items-baseline justify-between">
											<span class="text-body text-success-text font-medium">Change</span>
											<Numeric
												value={format.money(sale.changeDueMinor)}
												class="text-md text-success-text font-semibold"
											/>
										</div>
									{/if}
								</div>
							</div>
						{/if}
					</div>
				</Card>

				{#if sale && !settled}
					<Card label="Payment">
						<div class="flex flex-col gap-3">
							<Field id="cash" label="Cash received">
								{#snippet children({ id, describedBy })}
									<Input
										{id}
										{describedBy}
										bind:value={cashInput}
										numeric
										forceLtr
										placeholder="500"
									/>
								{/snippet}
							</Field>
							{#if split}
								<div class="border-border bg-surface-sunken mb-2 flex flex-col gap-2 border p-3">
									<div class="flex items-baseline justify-between">
										<span class="label-caps">Split {format.integer(splitWays)} ways</span>
										<Button variant="ghost" size="xs" onclick={() => (split = null)}>Cancel</Button>
									</div>

									{#each split as part (part.number)}
										<div class="flex items-center justify-between gap-2">
											<span class="text-secondary text-text-secondary">
												Share {format.integer(part.number)}
											</span>
											<Numeric value={format.moneyPlain(part.amountMinor)} />
											<Button
												variant="secondary"
												size="xs"
												onclick={() => tenderShare(part.amountMinor)}
												disabled={busy || balanceDue <= 0}
											>
												Take
											</Button>
										</div>
									{/each}

									<p class="text-secondary text-text-muted">
										Each share is an ordinary tender. The shares add up to the bill exactly — the
										odd minor unit goes to the earliest, because somebody has to absorb it.
									</p>
								</div>
							{:else if balanceDue > 0 && sale.lines.length > 0}
								<div class="mb-2 flex flex-wrap items-center gap-2">
									<span class="text-secondary text-text-muted">Split</span>
									{#each [2, 3, 4] as ways (ways)}
										<Button
											variant="ghost"
											size="xs"
											onclick={() => splitEvenly(ways)}
											disabled={busy}
										>
											{format.integer(ways)} ways
										</Button>
									{/each}
								</div>
							{/if}

							<!-- Laid out by weight rather than left to wrap. The two tender buttons are the
							     same kind of action, so they share a row at equal width; completing the sale is
							     the one thing this panel exists for, so it gets its own full-width row; and
							     abandoning is the only destructive action here, so it sits apart from the
							     button a cashier reaches for a hundred times a day. -->
							<div class="flex flex-col gap-2">
								<div class="grid grid-cols-2 gap-2">
									<Button
										variant="secondary"
										icon={Banknote}
										block
										onclick={tenderCash}
										disabled={busy}
									>
										Take cash
									</Button>
									<Button
										variant="secondary"
										icon={Banknote}
										block
										onclick={tenderExact}
										disabled={busy || balanceDue <= 0}
									>
										Exact
									</Button>
								</div>

								<Button
									variant="primary"
									icon={Check}
									block
									onclick={complete}
									disabled={busy || balanceDue > 0 || sale.lines.length === 0}
								>
									Complete sale
								</Button>

								<div class="border-border mt-1 border-t pt-2">
									<Button variant="ghost" icon={Ban} block onclick={abandon} disabled={busy}>
										Abandon sale
									</Button>
								</div>
							</div>
						</div>
					</Card>
				{:else if settled}
					<Card label="Done">
						<div class="flex flex-col gap-3">
							<p class="text-md">
								Paid <Numeric value={format.money(sale?.totalMinor ?? 0)} align="start" />
								{#if (sale?.changeDueMinor ?? 0) > 0}
									· change <Numeric value={format.money(sale?.changeDueMinor ?? 0)} align="start" />
								{/if}
							</p>

							{#if document?.regime === 'bd_mushak63'}
								<div class="border-border bg-surface-sunken border p-3">
									<p class="label-caps">Mushak 6.3</p>
									<div class="mt-1 flex items-baseline justify-between gap-3">
										<span class="text-secondary text-text-secondary">Challan no.</span>
										<Numeric value={document.invoiceNumber} class="font-semibold" />
									</div>
									<p class="text-secondary text-text-muted mt-1">
										{document.sellerName} · BIN {document.sellerBin}
									</p>
								</div>
							{:else if document?.regime === 'zatca'}
								<div class="border-border bg-surface-sunken border p-3">
									<p class="label-caps">Simplified tax invoice</p>
									<div class="mt-1 flex items-baseline justify-between gap-3">
										<span class="text-secondary text-text-secondary">Invoice no.</span>
										<Numeric value={document.invoiceNumber} class="font-semibold" />
									</div>
									<p class="text-secondary text-text-muted mt-1">
										{document.sellerName} · VAT {document.sellerVat}
									</p>
									<p class="text-secondary text-text-muted mt-1">
										The QR goes on the printed receipt — the till decides its bytes, so a screen
										redrawing it could disagree with the paper.
									</p>
								</div>
							{:else if documentProblem}
								<!-- The sale already went through. This says the challan could not be issued,
								     which is a thing to fix, not a thing to have refused the sale over. -->
								<div class="border-warn bg-warn-subtle text-warn-text border p-3">
									<p class="label-caps">No challan issued</p>
									<p class="text-secondary mt-1">{documentProblem}</p>
								</div>
							{/if}

							{#if hasPrinter}
								<div class="flex flex-col gap-2">
									<Button
										variant="secondary"
										size="lg"
										icon={Printer}
										block
										onclick={() => printReceipt(true)}
										disabled={busy}
									>
										Print receipt
									</Button>
									{#if printOutcome && !printOutcome.printed}
										<!-- The sale is done and the money is in the drawer. This is a thing to
										     fix, never a thing to have refused the sale over. -->
										<div class="border-warn bg-warn-subtle text-warn-text border p-3">
											<p class="label-caps">Not printed</p>
											<p class="text-secondary mt-1">{printOutcome.reason}</p>
										</div>
									{:else if printOutcome?.printed}
										<p class="text-secondary text-success-text">Sent to the printer.</p>
									{/if}
								</div>
							{/if}

							<Button variant="primary" size="lg" onclick={startSale} disabled={busy}>
								Next sale
							</Button>
						</div>
					</Card>
				{/if}

				{#if choosing}
					{@const product = choosing}
					<div
						class="bg-canvas/90 fixed inset-0 z-50 flex items-center justify-center p-6"
						role="dialog"
						aria-modal="true"
						aria-label="Choose options"
					>
						<div
							class="border-border bg-surface flex max-h-[85dvh] w-full max-w-md flex-col border"
						>
							<div class="border-border shrink-0 border-b p-4">
								<p class="label-caps">Options</p>
								<p class="text-md mt-1">{product.name}</p>
							</div>

							<div class="min-h-0 flex-1 overflow-y-auto p-4">
								<div class="flex flex-col gap-4">
									{#each product.optionGroups as group (group.id)}
										<div class="flex flex-col gap-2">
											<div class="flex items-baseline justify-between gap-2">
												<span class="text-body font-medium">{group.name}</span>
												<span class="text-secondary text-text-muted">
													{#if group.min > 0 && group.max === 1}
														Choose one
													{:else if group.max === 1}
														Choose up to one
													{:else if group.min > 0}
														Choose {format.integer(group.min)} to {format.integer(group.max)}
													{:else}
														Choose up to {format.integer(group.max)}
													{/if}
												</span>
											</div>

											<div class="grid grid-cols-2 gap-2">
												{#each group.options as option (option.id)}
													{@const picked = chosen.includes(option.id)}
													<button
														type="button"
														onclick={() => toggleChoice(group, option.id)}
														class="flex flex-col items-start gap-0.5 border p-2 text-start transition-colors
											       {picked
															? 'border-primary bg-primary-subtle'
															: 'border-border bg-surface hover:bg-surface-hover'}"
														style="min-height: var(--scale-control-height)"
													>
														<span class="text-body">{option.name}</span>
														{#if option.priceDeltaMinor !== 0}
															<span class="numeric text-secondary text-text-secondary">
																{option.priceDeltaMinor > 0 ? '+' : '−'}{format.moneyPlain(
																	Math.abs(option.priceDeltaMinor)
																)}
															</span>
														{/if}
													</button>
												{/each}
											</div>
										</div>
									{/each}
								</div>
							</div>

							<div class="border-border flex shrink-0 gap-2 border-t p-4">
								<Button
									variant="primary"
									size="lg"
									block
									onclick={confirmChoices}
									disabled={busy || !choicesComplete(product, chosen)}
								>
									Add to sale
								</Button>
								<Button variant="ghost" size="lg" onclick={() => (choosing = null)} disabled={busy}>
									Cancel
								</Button>
							</div>
						</div>
					</div>
				{/if}

				{#if pendingVoid}
					<PinPrompt
						action="Void a line from this sale"
						{busy}
						error={approvalError}
						onsubmit={confirmVoid}
						oncancel={() => {
							pendingVoid = null;
							approvalError = null;
						}}
					/>
				{/if}

				{#if error}
					<!-- role="alert" so a refusal is announced, phrased for a cashier not a developer. -->
					<div
						role="alert"
						class="border-danger bg-danger-subtle text-body text-danger-text border p-3"
					>
						{error.message}
					</div>
				{/if}
			</div>
		</div>
	{/if}
</div>
