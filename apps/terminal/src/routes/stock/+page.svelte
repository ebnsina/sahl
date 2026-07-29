<script lang="ts">
	/**
	 * The stock screen: receive a delivery, count a shelf, write stock off.
	 *
	 * The blind count works exactly as it does on the drawer, and for the same reason — someone
	 * counting a shelf while looking at what the book expects is confirming a number rather than
	 * counting stock. `blindStockSheet` strips the recorded levels in Rust before they leave the
	 * process, so this component never holds them while a count is open.
	 *
	 * Quantities are thousandths of a unit throughout. Nothing here converts one; `parseQuantity`
	 * decodes a typed string and `Intl` renders it back.
	 */
	import {
		Badge,
		Button,
		Card,
		Field,
		Input,
		Numeric,
		createFormatters,
		parseMinor
	} from '@sahl/ui';
	import {
		asTillError,
		isTillAvailable,
		till,
		type BatchView,
		type IssueReason,
		type StockView
	} from '$lib/till';

	const STAFF = '00000000-0000-0000-0000-0000000000ca';

	const format = createFormatters({ locale: 'en', currency: 'BDT', timeZone: 'Asia/Dhaka' });

	// A stand-in catalogue until the real one lands, matching the sell screen's ids.
	const PRODUCTS = [
		{ id: '00000000-0000-0000-0000-000000000101', name: 'Basmati rice 5kg' },
		{ id: '00000000-0000-0000-0000-000000000102', name: 'Cooking oil 2L' },
		{ id: '00000000-0000-0000-0000-000000000103', name: 'Bread' },
		{ id: '00000000-0000-0000-0000-000000000104', name: 'Fresh milk 1L' },
		{ id: '00000000-0000-0000-0000-000000000105', name: 'Lentils 1kg' },
		{ id: '00000000-0000-0000-0000-000000000106', name: 'Tea 400g' }
	];

	const ISSUE_REASONS: Array<{ value: IssueReason; label: string }> = [
		{ value: 'wastage', label: 'Wastage — spoiled or broken' },
		{ value: 'transfer_out', label: 'Sent to another outlet' },
		{ value: 'return_to_supplier', label: 'Returned to supplier' },
		{ value: 'internal', label: 'Taken for shop use' }
	];

	let stock = $state<StockView | null>(null);
	/** Levels withheld. Drives the counting panel and nothing else. */
	let sheet = $state<StockView | null>(null);
	let error = $state<{ code: string; message: string } | null>(null);
	let busy = $state(false);
	let available = $state(true);

	let productId = $state(PRODUCTS[0]?.id ?? '');
	let lot = $state('');
	let expiry = $state('');
	let quantity = $state('');
	let unitCost = $state('');
	let supplier = $state('');

	let counting = $state(false);
	let countBatch = $state<string | null>(null);
	let countInput = $state('');

	let issueBatch = $state<string | null>(null);
	let issueQuantity = $state('');
	let issueReason = $state<IssueReason>('wastage');

	let negatives = $derived(stock?.batches.filter((batch) => batch.negative) ?? []);
	let shortfalls = $derived(stock?.variances.filter((variance) => variance.deltaMilli < 0) ?? []);

	async function run<T>(action: () => Promise<T>, onDone?: (result: T) => void) {
		busy = true;
		error = null;
		try {
			onDone?.(await action());
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
				() => till.stockPosition(),
				(result) => (stock = result)
			);
		}
	});

	/**
	 * Decode a typed quantity into thousandths.
	 *
	 * Separate from `parseMinor` because quantity has a fixed three-decimal scale rather than a
	 * per-currency one, and no currency to check it against. Same shape otherwise: digit
	 * manipulation, no float, `null` rather than a guess.
	 */
	function parseQuantity(entry: string): number | null {
		const trimmed = entry.trim();
		if (!/^\d+(\.\d{1,3})?$/.test(trimmed)) return null;
		const [whole = '0', fraction = ''] = trimmed.split('.');
		const value = Number(`${whole}${fraction.padEnd(3, '0')}`);
		return Number.isSafeInteger(value) ? value : null;
	}

	function receive() {
		const quantityMilli = parseQuantity(quantity);
		if (quantityMilli === null || quantityMilli <= 0) {
			error = { code: 'bad_quantity', message: 'Enter a quantity like 10 or 2.5' };
			return;
		}
		const unitCostMinor = parseMinor(unitCost, 'BDT');
		if (unitCostMinor === null || unitCostMinor < 0) {
			error = { code: 'bad_amount', message: 'Enter a unit cost like 40 or 39.50' };
			return;
		}

		// An empty date is "no expiry", which is correct for rice and wrong to invent for milk —
		// so it is left absent rather than defaulted to some far-off day.
		let expiresAtMillis: number | null = null;
		if (expiry.trim()) {
			const parsed = Date.parse(`${expiry}T00:00:00Z`);
			if (Number.isNaN(parsed)) {
				error = {
					code: 'bad_date',
					message: 'Enter an expiry date as YYYY-MM-DD, or leave it blank'
				};
				return;
			}
			expiresAtMillis = parsed;
		}

		void run(
			() =>
				till.receiveStock({
					productId,
					lot: lot.trim() || null,
					expiresAtMillis,
					quantityMilli,
					unitCostMinor,
					supplier: supplier.trim() || null,
					receivedBy: STAFF
				}),
			(result) => {
				stock = result;
				lot = '';
				expiry = '';
				quantity = '';
				unitCost = '';
			}
		);
	}

	function startCount(batchId: string) {
		void run(
			() => till.blindStockSheet(),
			(result) => {
				sheet = result;
				countBatch = batchId;
				countInput = '';
				counting = true;
			}
		);
	}

	function submitCount() {
		const batchId = countBatch;
		const countedMilli = parseQuantity(countInput);
		if (!batchId) return;
		if (countedMilli === null) {
			error = { code: 'bad_quantity', message: 'Enter what you counted, like 9 or 8.75' };
			return;
		}
		void run(
			() => till.countStock(batchId, countedMilli, STAFF),
			(result) => {
				stock = result;
				sheet = null;
				counting = false;
				countBatch = null;
				countInput = '';
			}
		);
	}

	function submitIssue() {
		const batchId = issueBatch;
		const quantityMilli = parseQuantity(issueQuantity);
		if (!batchId) return;
		if (quantityMilli === null || quantityMilli <= 0) {
			error = { code: 'bad_quantity', message: 'Enter a positive quantity' };
			return;
		}
		void run(
			() => till.issueStock(batchId, quantityMilli, issueReason, STAFF),
			(result) => {
				stock = result;
				issueBatch = null;
				issueQuantity = '';
			}
		);
	}

	function productName(id: string): string {
		return PRODUCTS.find((product) => product.id === id)?.name ?? 'Unknown product';
	}

	function expiryTone(batch: BatchView): 'danger' | 'warn' | 'neutral' {
		if (batch.expiresAt === null) return 'neutral';
		const days = (batch.expiresAt - Date.now()) / 86_400_000;
		if (days < 0) return 'danger';
		return days < 7 ? 'warn' : 'neutral';
	}
</script>

<svelte:head><title>Stock — Sahl</title></svelte:head>

<main class="bg-canvas text-text flex min-h-dvh flex-col" data-density="touch">
	<header class="border-border bg-surface flex items-center justify-between border-b px-4 py-3">
		<div class="flex items-center gap-3">
			<h1 class="text-lg font-semibold">Stock</h1>
			{#if negatives.length > 0}
				<Badge tone="danger" dot>{format.integer(negatives.length)} below zero</Badge>
			{/if}
			{#if shortfalls.length > 0}
				<Badge tone="warn">{format.integer(shortfalls.length)} short on count</Badge>
			{/if}
		</div>
		<div class="flex gap-4">
			<a href="/orders" class="text-secondary text-text-secondary hover:text-text underline">
				Orders
			</a>
			<a href="/shift" class="text-secondary text-text-secondary hover:text-text underline">Shift</a
			>
			<a href="/" class="text-secondary text-text-secondary hover:text-text underline">
				Sell screen
			</a>
		</div>
	</header>

	{#if !available}
		<div class="flex flex-1 items-center justify-center p-8">
			<Card label="Not connected" class="max-w-lg">
				<p class="text-md">This screen runs inside the Sahl till application.</p>
			</Card>
		</div>
	{:else}
		<div class="grid flex-1 grid-cols-1 gap-4 overflow-y-auto p-4 lg:grid-cols-[1fr_24rem]">
			<div class="flex flex-col gap-4">
				<Card label="On hand" flush>
					{#if !stock || stock.batches.length === 0}
						<p class="text-secondary text-text-muted p-4">
							Nothing received yet. Book a delivery in on the right.
						</p>
					{:else}
						{#each stock.batches as batch (batch.id)}
							<div
								class="border-border flex items-center gap-3 border-b px-3"
								style="min-height: var(--scale-row-height)"
							>
								<div class="min-w-0 flex-1">
									<p class="text-body truncate">{productName(batch.productId)}</p>
									<p class="text-secondary text-text-muted">
										{batch.lot ?? 'No lot'} · received {format.date(batch.receivedAt)}
									</p>
								</div>

								{#if batch.expiresAt !== null}
									<Badge tone={expiryTone(batch)}>
										{expiryTone(batch) === 'danger' ? 'Expired' : format.date(batch.expiresAt)}
									</Badge>
								{/if}

								{#if batch.negative}
									<Badge tone="danger" dot>Below zero</Badge>
								{/if}

								<Numeric value={format.quantity(batch.onHandMilli)} />

								<div class="flex gap-1">
									<Button
										variant="ghost"
										size="xs"
										onclick={() => startCount(batch.id)}
										disabled={busy}
									>
										Count
									</Button>
									<Button
										variant="ghost"
										size="xs"
										onclick={() => {
											issueBatch = batch.id;
											issueQuantity = '';
										}}
										disabled={busy}
									>
										Issue
									</Button>
								</div>
							</div>
						{/each}
					{/if}
				</Card>

				{#if stock && stock.variances.length > 0}
					<Card label="Count variances">
						<p class="text-secondary text-text-secondary mb-3">
							One batch off a little is noise. The same batch off every count is the thing worth
							reading.
						</p>
						{#each stock.variances as variance (variance.batchId + variance.at)}
							<div class="border-border flex items-center justify-between gap-3 border-b py-2">
								<div class="min-w-0">
									<p class="text-body truncate">
										{productName(
											stock.batches.find((batch) => batch.id === variance.batchId)?.productId ?? ''
										)}
									</p>
									<p class="text-secondary text-text-muted">{format.dateTime(variance.at)}</p>
								</div>
								<div class="flex items-center gap-3">
									<span class="text-secondary text-text-muted">
										expected {format.quantity(variance.expectedMilli)}
									</span>
									<Badge tone={variance.deltaMilli < 0 ? 'danger' : 'warn'}>
										{variance.deltaMilli < 0 ? 'Short' : 'Over'}
										{format.quantity(Math.abs(variance.deltaMilli))}
									</Badge>
								</div>
							</div>
						{/each}
					</Card>
				{/if}
			</div>

			<div class="flex flex-col gap-4">
				{#if counting && sheet && countBatch}
					<Card label="Count the shelf">
						<div class="flex flex-col gap-4">
							<p class="text-secondary text-text-secondary">
								Counting <strong
									>{productName(
										sheet.batches.find((batch) => batch.id === countBatch)?.productId ?? ''
									)}</strong
								>. The recorded level is deliberately not shown — counting towards a number is not
								counting.
							</p>
							<Field id="counted-stock" label="Counted quantity">
								{#snippet children({ id, describedBy })}
									<Input
										{id}
										{describedBy}
										bind:value={countInput}
										numeric
										forceLtr
										placeholder="9"
									/>
								{/snippet}
							</Field>
							<div class="flex gap-2">
								<Button variant="primary" size="lg" onclick={submitCount} disabled={busy}>
									Record count
								</Button>
								<Button
									variant="ghost"
									size="lg"
									onclick={() => {
										counting = false;
										sheet = null;
										countBatch = null;
									}}
									disabled={busy}>Cancel</Button
								>
							</div>
						</div>
					</Card>
				{:else if issueBatch}
					<Card label="Issue stock">
						<div class="flex flex-col gap-3">
							<Field id="issue-reason" label="Reason">
								{#snippet children({ id, describedBy })}
									<select
										{id}
										aria-describedby={describedBy}
										bind:value={issueReason}
										class="border-border bg-surface text-body w-full border px-3"
										style="min-height: var(--scale-touch-target)"
									>
										{#each ISSUE_REASONS as reason (reason.value)}
											<option value={reason.value}>{reason.label}</option>
										{/each}
									</select>
								{/snippet}
							</Field>
							<Field id="issue-quantity" label="Quantity">
								{#snippet children({ id, describedBy })}
									<Input
										{id}
										{describedBy}
										bind:value={issueQuantity}
										numeric
										forceLtr
										placeholder="1"
									/>
								{/snippet}
							</Field>
							<div class="flex gap-2">
								<Button variant="danger" size="lg" onclick={submitIssue} disabled={busy}>
									Issue stock
								</Button>
								<Button
									variant="ghost"
									size="lg"
									onclick={() => (issueBatch = null)}
									disabled={busy}
								>
									Cancel
								</Button>
							</div>
						</div>
					</Card>
				{:else}
					<Card label="Receive a delivery">
						<div class="flex flex-col gap-3">
							<p class="text-secondary text-text-secondary">
								Each delivery becomes its own batch. A second delivery of the same product is a
								different lot with its own expiry — merging them is what makes a recall
								under-report.
							</p>

							<Field id="receive-product" label="Product">
								{#snippet children({ id, describedBy })}
									<select
										{id}
										aria-describedby={describedBy}
										bind:value={productId}
										class="border-border bg-surface text-body w-full border px-3"
										style="min-height: var(--scale-touch-target)"
									>
										{#each PRODUCTS as product (product.id)}
											<option value={product.id}>{product.name}</option>
										{/each}
									</select>
								{/snippet}
							</Field>

							<Field id="receive-quantity" label="Quantity">
								{#snippet children({ id, describedBy })}
									<Input
										{id}
										{describedBy}
										bind:value={quantity}
										numeric
										forceLtr
										placeholder="10"
									/>
								{/snippet}
							</Field>

							<Field
								id="receive-cost"
								label="Unit cost"
								hint="What was actually charged, not the quoted price."
							>
								{#snippet children({ id, describedBy })}
									<Input
										{id}
										{describedBy}
										bind:value={unitCost}
										numeric
										forceLtr
										placeholder="40"
									/>
								{/snippet}
							</Field>

							<Field id="receive-lot" label="Lot" hint="Optional. What a recall traces along.">
								{#snippet children({ id, describedBy })}
									<Input {id} {describedBy} bind:value={lot} placeholder="KT-4471" />
								{/snippet}
							</Field>

							<Field
								id="receive-expiry"
								label="Expires"
								hint="Leave blank if it does not expire — YYYY-MM-DD."
							>
								{#snippet children({ id, describedBy })}
									<Input {id} {describedBy} bind:value={expiry} forceLtr placeholder="2026-12-31" />
								{/snippet}
							</Field>

							<Field id="receive-supplier" label="Supplier" hint="Optional.">
								{#snippet children({ id, describedBy })}
									<Input {id} {describedBy} bind:value={supplier} placeholder="Karim Traders" />
								{/snippet}
							</Field>

							<Button variant="primary" size="lg" onclick={receive} disabled={busy}>
								Receive delivery
							</Button>
						</div>
					</Card>
				{/if}

				{#if negatives.length > 0}
					<Card label="Below zero">
						<p class="text-secondary text-text-secondary">
							Stock left that the book never saw arrive — a delivery not entered, or an issue
							recorded twice. A count clears it.
						</p>
					</Card>
				{/if}
			</div>
		</div>
	{/if}

	{#if error}
		<div class="border-danger-border bg-danger-surface text-danger-text border-t px-4 py-3">
			<p class="text-body">{error.message}</p>
		</div>
	{/if}
</main>
