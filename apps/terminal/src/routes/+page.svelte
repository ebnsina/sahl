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
	import { Badge, Button, Card, Field, Input, Numeric, createFormatters } from '@sahl/ui';
	import {
		asTillError,
		isTillAvailable,
		till,
		type SaleView,
		type SyncView,
		type TillStatus
	} from '$lib/till';

	// A stand-in catalogue until the real one lands. Prices are tax-inclusive minor units.
	const CATALOGUE = [
		{
			id: '00000000-0000-0000-0000-000000000101',
			name: 'Basmati rice 5kg',
			minor: 48_000,
			bp: 1500
		},
		{ id: '00000000-0000-0000-0000-000000000102', name: 'Cooking oil 2L', minor: 34_000, bp: 1500 },
		{ id: '00000000-0000-0000-0000-000000000103', name: 'Bread', minor: 5_500, bp: 750 },
		{ id: '00000000-0000-0000-0000-000000000104', name: 'Fresh milk 1L', minor: 9_000, bp: 0 },
		{ id: '00000000-0000-0000-0000-000000000105', name: 'Lentils 1kg', minor: 14_500, bp: 750 },
		{ id: '00000000-0000-0000-0000-000000000106', name: 'Tea 400g', minor: 32_000, bp: 1500 }
	];

	const CASHIER = '00000000-0000-0000-0000-0000000000ca';
	const MANAGER = '00000000-0000-0000-0000-00000000011a';

	const format = createFormatters({ locale: 'en', currency: 'BDT', timeZone: 'Asia/Dhaka' });

	let sale = $state<SaleView | null>(null);
	let status = $state<TillStatus | null>(null);
	let sync = $state<SyncView | null>(null);
	let error = $state<{ code: string; message: string } | null>(null);
	let busy = $state(false);
	let cashInput = $state('');
	let available = $state(true);

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
		}
	});

	function startSale() {
		void run(
			() => till.openSale(CASHIER),
			(result) => {
				sale = result;
				cashInput = '';
			}
		);
	}

	function addItem(item: (typeof CATALOGUE)[number]) {
		const current = sale;
		if (!current) return;
		void run(
			() =>
				till.addLine({
					saleId: current.id,
					productId: item.id,
					name: item.name,
					unitPriceMinor: item.minor,
					quantityMilli: 1000,
					taxBasisPoints: item.bp,
					currency: 'BDT'
				}),
			(result) => (sale = result)
		);
	}

	function voidLine(lineId: string) {
		const current = sale;
		if (!current) return;
		void run(
			() => till.voidLine(current.id, lineId, 'mistake', MANAGER),
			(result) => (sale = result)
		);
	}

	/**
	 * Parse a cash entry into exact minor units without floating point.
	 *
	 * `parseFloat(entry) * 100` is the obvious version and it is wrong: 19.99 becomes 1998.9999…
	 * and truncates a taka short. Splitting on the decimal point and padding keeps it exact. This is
	 * parsing, not arithmetic on a monetary value — the amount goes straight to Rust untouched.
	 */
	function toMinor(entry: string): number | null {
		const trimmed = entry.trim();
		if (!/^\d+(\.\d{0,2})?$/.test(trimmed)) return null;
		const [whole = '0', fraction = ''] = trimmed.split('.');
		return Number(whole) * 100 + Number(fraction.padEnd(2, '0'));
	}

	function tenderCash() {
		const current = sale;
		if (!current) return;
		const amountMinor = toMinor(cashInput);
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
			() => till.completeSale(current.id),
			(result) => (sale = result)
		);
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

<div data-density="touch" class="bg-canvas text-text flex h-screen flex-col">
	<header class="border-border bg-surface flex shrink-0 items-center gap-3 border-b px-4 py-2.5">
		<span class="text-md font-semibold">Sahl</span>

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
				<div class="grid grid-cols-2 gap-3 sm:grid-cols-3">
					{#each CATALOGUE as item (item.id)}
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
							<Numeric value={format.money(item.minor)} align="start" class="text-secondary" />
							{#if item.bp === 0}
								<Badge tone="neutral">Exempt</Badge>
							{:else}
								<Badge tone="neutral">{format.percent(item.bp)} VAT</Badge>
							{/if}
						</button>
					{/each}
				</div>
			</Card>

			<div class="flex min-h-0 flex-col gap-3">
				<Card label="Sale" flush class="flex min-h-0 flex-1 flex-col">
					<div class="flex min-h-0 flex-1 flex-col">
						{#if !sale}
							<div class="flex flex-1 flex-col items-center justify-center gap-3 p-6">
								<p class="text-secondary text-text-muted">No sale in progress.</p>
								<Button variant="primary" size="lg" onclick={startSale} disabled={busy}>
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
													onclick={() => voidLine(line.id)}
													disabled={busy}>Void</Button
												>
											{/if}
										{/if}
									</div>
								{/each}
							</div>

							<div class="border-border bg-surface-sunken shrink-0 border-t p-3">
								<div class="flex flex-col gap-1">
									{#each sale.taxGroups as group (group.class + group.basisPoints)}
										<div class="text-secondary text-text-secondary flex justify-between">
											<span>
												{group.class === 'exempt'
													? 'Exempt'
													: group.class === 'zero_rated'
														? 'Zero-rated'
														: `VAT ${format.percent(group.basisPoints)}`}
											</span>
											<Numeric value={format.moneyPlain(group.taxMinor)} class="text-secondary" />
										</div>
									{/each}

									<div class="border-border mt-1 flex items-baseline justify-between border-t pt-2">
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
							<div class="flex flex-wrap gap-2">
								<Button variant="secondary" onclick={tenderCash} disabled={busy}>Take cash</Button>
								<Button
									variant="secondary"
									onclick={tenderExact}
									disabled={busy || balanceDue <= 0}
								>
									Exact
								</Button>
								<Button
									variant="primary"
									onclick={complete}
									disabled={busy || balanceDue > 0 || sale.lines.length === 0}
								>
									Complete sale
								</Button>
								<Button variant="ghost" onclick={abandon} disabled={busy}>Abandon</Button>
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
							<Button variant="primary" size="lg" onclick={startSale} disabled={busy}>
								Next sale
							</Button>
						</div>
					</Card>
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
