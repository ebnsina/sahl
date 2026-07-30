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
	import Banknote from '@lucide/svelte/icons/banknote';
	import Check from '@lucide/svelte/icons/check';
	import Plus from '@lucide/svelte/icons/plus';
	import Trash2 from '@lucide/svelte/icons/trash-2';
	import {
		Badge,
		Button,
		Card,
		Field,
		Input,
		Numeric,
		Logo,
		createFormatters,
		parseMinor
	} from '@sahl/ui';
	import PinPrompt from '$lib/PinPrompt.svelte';
	import {
		asTillError,
		isTillAvailable,
		till,
		type SaleView,
		type DocumentView,
		type PrintOutcome,
		type SyncView,
		type ProductView,
		type TillStatus
	} from '$lib/till';

	/** The real catalogue, from the till. Empty until someone adds a product. */
	let catalogue = $state<ProductView[]>([]);

	const CASHIER = '00000000-0000-0000-0000-0000000000ca';

	const format = createFormatters({ locale: 'en', currency: 'BDT', timeZone: 'Asia/Dhaka' });

	let sale = $state<SaleView | null>(null);
	/** The challan for the sale just completed, if this outlet issues one. */
	let document = $state<DocumentView | null>(null);
	/** Why a challan could not be issued. Surfaced, never allowed to block the sale. */
	let documentProblem = $state<string | null>(null);
	let hasPrinter = $state(false);
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
		} catch (thrown) {
			error = asTillError(thrown);
			if (error.code === 'no_till') available = false;
		} finally {
			busy = false;
		}
	}

	$effect(() => {
		available = isTillAvailable();
		if (available) {
			void run(
				() => till.status(),
				(result) => (status = result)
			);
			void till.printerConfigured().then((configured) => (hasPrinter = configured));
			void run(
				() => till.sellableProducts(),
				(result) => (catalogue = result)
			);
		}
	});

	function startSale() {
		document = null;
		documentProblem = null;
		printOutcome = null;
		void run(
			() => till.openSale(CASHIER),
			(result) => {
				sale = result;
				cashInput = '';
			}
		);
	}

	function addItem(item: ProductView) {
		const current = sale;
		if (!current) return;
		void run(
			() =>
				till.addLine({
					saleId: current.id,
					productId: item.id,
					name: item.name,
					unitPriceMinor: item.priceMinor,
					// One unit per tap. A divisible product still needs a real quantity, which is what
					// a scale or a keypad will supply — tapping it is not a weighing.
					quantityMilli: 1000,
					taxBasisPoints: item.taxBasisPoints,
					taxTreatment: item.taxTreatment,
					currency: 'BDT'
				}),
			(result) => (sale = result)
		);
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

	function tenderCash() {
		const current = sale;
		if (!current) return;
		const amountMinor = parseMinor(cashInput, 'BDT');
		if (amountMinor === null || amountMinor <= 0) {
			error = { code: 'bad_amount', message: 'Enter a cash amount like 500 or 499.50' };
			return;
		}
		void run(
			() => till.recordTender({ saleId: current.id, method: 'cash', amountMinor, currency: 'BDT' }),
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
					amountMinor: current.balanceDueMinor,
					currency: 'BDT'
				}),
			(result) => (sale = result)
		);
	}

	function complete() {
		const current = sale;
		if (!current) return;
		void run(
			() => till.completeSale(current.id, CASHIER),
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
			() => till.abandonSale(current.id, CASHIER),
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
				<span class="label-caps">Takings</span>
				<Numeric value={format.money(status.takingsMinor)} class="font-semibold" />
			{/if}
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
	{:else}
		<div class="grid flex-1 grid-cols-1 gap-4 overflow-hidden p-4 lg:grid-cols-[1fr_26rem]">
			<Card label="Items" class="flex min-h-0 flex-col">
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
							</div>
						</button>
					{/each}
				</div>
			</Card>

			<div class="flex min-h-0 flex-col gap-3">
				<Card
					label="Sale"
					flush
					class="flex min-h-0 flex-1 flex-col"
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
