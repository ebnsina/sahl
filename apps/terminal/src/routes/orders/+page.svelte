<script lang="ts">
	/**
	 * Purchase orders: place one, book deliveries against it, close it.
	 *
	 * The difference between this and receiving on the stock screen is the whole reason orders
	 * exist. The batch ledger records what arrived and is perfectly consistent whatever that was —
	 * fifty kilos ordered and thirty delivered leaves it recording a contented thirty. Only the
	 * order knows twenty are missing, and only the order knows the price changed on the way.
	 */
	import {
		Badge,
		Button,
		Card,
		Field,
		Input,
		Numeric,
		createFormatters,
		minorToDecimalString,
		parseMinor
	} from '@sahl/ui';
	import {
		asTillError,
		isTillAvailable,
		till,
		type CloseReason,
		type OrderLineView,
		type OrderView
	} from '$lib/till';

	const STAFF = '00000000-0000-0000-0000-0000000000ca';

	const format = createFormatters({ locale: 'en', currency: 'BDT', timeZone: 'Asia/Dhaka' });

	// A stand-in catalogue until the real one lands, matching the other screens' ids.
	const PRODUCTS = [
		{ id: '00000000-0000-0000-0000-000000000101', name: 'Basmati rice 5kg' },
		{ id: '00000000-0000-0000-0000-000000000102', name: 'Cooking oil 2L' },
		{ id: '00000000-0000-0000-0000-000000000103', name: 'Bread' },
		{ id: '00000000-0000-0000-0000-000000000104', name: 'Fresh milk 1L' },
		{ id: '00000000-0000-0000-0000-000000000105', name: 'Lentils 1kg' },
		{ id: '00000000-0000-0000-0000-000000000106', name: 'Tea 400g' }
	];

	const CLOSE_REASONS: Array<{ value: CloseReason; label: string }> = [
		{ value: 'complete', label: 'Everything arrived' },
		{ value: 'short_shipped', label: 'The rest is not coming' },
		{ value: 'cancelled', label: 'Called off before delivery' }
	];

	let orders = $state<OrderView[]>([]);
	let error = $state<{ code: string; message: string } | null>(null);
	let busy = $state(false);
	let available = $state(true);

	let supplier = $state('');
	let reference = $state('');
	let expected = $state('');
	let draftLines = $state<Array<{ productId: string; quantity: string; unitCost: string }>>([
		{ productId: PRODUCTS[0]?.id ?? '', quantity: '', unitCost: '' }
	]);

	/** The line being received against, if any. */
	let receiving = $state<{ orderId: string; line: OrderLineView } | null>(null);
	let receiveQuantity = $state('');
	let receiveCost = $state('');
	let receiveLot = $state('');
	let receiveExpiry = $state('');

	let closing = $state<string | null>(null);
	let closeReason = $state<CloseReason>('complete');

	let open = $derived(orders.filter((order) => order.status !== 'closed'));
	let settled = $derived(orders.filter((order) => order.status === 'closed'));
	let shortOrders = $derived(
		open.filter((order) => order.lines.some((line) => line.outstandingMilli > 0))
	);

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
				() => till.orderList(),
				(result) => (orders = result)
			);
		}
	});

	/** Decode a typed quantity into thousandths. Digit manipulation, no float. */
	function parseQuantity(entry: string): number | null {
		const trimmed = entry.trim();
		if (!/^\d+(\.\d{1,3})?$/.test(trimmed)) return null;
		const [whole = '0', fraction = ''] = trimmed.split('.');
		const value = Number(`${whole}${fraction.padEnd(3, '0')}`);
		return Number.isSafeInteger(value) ? value : null;
	}

	/** A date entry, or `null` when blank. Blank means "no date", never a made-up one. */
	function parseDate(entry: string): number | null | 'invalid' {
		if (!entry.trim()) return null;
		const parsed = Date.parse(`${entry}T00:00:00Z`);
		return Number.isNaN(parsed) ? 'invalid' : parsed;
	}

	function place() {
		const lines: Array<{ productId: string; quantityMilli: number; unitCostMinor: number }> = [];
		for (const draft of draftLines) {
			const quantityMilli = parseQuantity(draft.quantity);
			const unitCostMinor = parseMinor(draft.unitCost, 'BDT');
			if (quantityMilli === null || quantityMilli <= 0) {
				error = { code: 'bad_quantity', message: 'Every line needs a quantity like 10 or 2.5' };
				return;
			}
			if (unitCostMinor === null || unitCostMinor < 0) {
				error = { code: 'bad_amount', message: 'Every line needs a unit cost like 40 or 39.50' };
				return;
			}
			lines.push({ productId: draft.productId, quantityMilli, unitCostMinor });
		}

		const expectedAtMillis = parseDate(expected);
		if (expectedAtMillis === 'invalid') {
			error = {
				code: 'bad_date',
				message: 'Enter the expected date as YYYY-MM-DD, or leave it blank'
			};
			return;
		}

		void run(
			() =>
				till.placeOrder({
					supplier,
					reference: reference.trim() || null,
					expectedAtMillis,
					lines,
					placedBy: STAFF
				}),
			(result) => {
				orders = result;
				supplier = '';
				reference = '';
				expected = '';
				draftLines = [{ productId: PRODUCTS[0]?.id ?? '', quantity: '', unitCost: '' }];
			}
		);
	}

	function startReceive(orderId: string, line: OrderLineView) {
		receiving = { orderId, line };
		// Prefilled with what is still outstanding and the price agreed — the common case is that
		// the delivery matches, and retyping it is where a wrong number comes from. Rendered by
		// digit manipulation, never division: `unitCostMinor / 100` is the float this codebase
		// exists to avoid, and it would round a prefilled price before anyone saw it.
		receiveQuantity = minorToDecimalString(line.outstandingMilli, 3);
		receiveCost = minorToDecimalString(line.unitCostMinor, 2);
		receiveLot = '';
		receiveExpiry = '';
	}

	function submitReceive() {
		const target = receiving;
		if (!target) return;

		const quantityMilli = parseQuantity(receiveQuantity);
		const unitCostMinor = parseMinor(receiveCost, 'BDT');
		if (quantityMilli === null || quantityMilli <= 0) {
			error = { code: 'bad_quantity', message: 'Enter what arrived, like 30 or 12.5' };
			return;
		}
		if (unitCostMinor === null || unitCostMinor < 0) {
			error = { code: 'bad_amount', message: 'Enter what was charged, like 40 or 39.50' };
			return;
		}
		const expiresAtMillis = parseDate(receiveExpiry);
		if (expiresAtMillis === 'invalid') {
			error = { code: 'bad_date', message: 'Enter the expiry as YYYY-MM-DD, or leave it blank' };
			return;
		}

		void run(
			() =>
				till.receiveAgainstOrder({
					orderId: target.orderId,
					lineId: target.line.lineId,
					quantityMilli,
					unitCostMinor,
					lot: receiveLot.trim() || null,
					expiresAtMillis,
					receivedBy: STAFF
				}),
			(result) => {
				orders = result;
				receiving = null;
			}
		);
	}

	function submitClose() {
		const orderId = closing;
		if (!orderId) return;
		void run(
			() => till.closeOrder(orderId, closeReason, STAFF),
			(result) => {
				orders = result;
				closing = null;
			}
		);
	}

	function productName(id: string): string {
		return PRODUCTS.find((product) => product.id === id)?.name ?? 'Unknown product';
	}

	function statusLabel(order: OrderView): string {
		switch (order.status) {
			case 'awaiting':
				return 'Awaiting delivery';
			case 'partly_received':
				return 'Partly received';
			case 'fully_received':
				return 'All arrived';
			default:
				return closeLabel(order.closeReason);
		}
	}

	function closeLabel(reason: OrderView['closeReason']): string {
		switch (reason) {
			case 'complete':
				return 'Closed — complete';
			case 'short_shipped':
				return 'Closed — short shipped';
			case 'cancelled':
				return 'Cancelled';
			default:
				return 'Closed';
		}
	}

	function statusTone(order: OrderView): 'success' | 'warn' | 'neutral' | 'primary' {
		switch (order.status) {
			case 'fully_received':
				return 'success';
			case 'partly_received':
				return 'warn';
			case 'closed':
				return order.closeReason === 'complete' ? 'neutral' : 'warn';
			default:
				return 'primary';
		}
	}
</script>

<svelte:head><title>Orders — Sahl</title></svelte:head>

<main class="bg-canvas text-text flex min-h-dvh flex-col" data-density="touch">
	<header class="border-border bg-surface flex items-center justify-between border-b px-4 py-3">
		<div class="flex items-center gap-3">
			<h1 class="text-lg font-semibold">Orders</h1>
			{#if shortOrders.length > 0}
				<Badge tone="warn">{format.integer(shortOrders.length)} awaiting stock</Badge>
			{/if}
		</div>
		<div class="flex gap-4">
			<a href="/stock" class="text-secondary text-text-secondary hover:text-text underline">Stock</a
			>
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
				{#if orders.length === 0}
					<Card label="No orders">
						<p class="text-secondary text-text-muted">
							Nothing ordered yet. An order is what lets the till tell a short delivery from a small
							one.
						</p>
					</Card>
				{/if}

				{#each [...open, ...settled] as order (order.id)}
					<Card label={order.supplier}>
						<div class="flex flex-col gap-3">
							<div class="flex flex-wrap items-center justify-between gap-2">
								<div class="min-w-0">
									<p class="text-secondary text-text-muted">
										{order.reference ?? 'No reference'} · placed {format.date(order.placedAt)}
										{#if order.expectedAt !== null}
											· expected {format.date(order.expectedAt)}
										{/if}
									</p>
								</div>
								<Badge tone={statusTone(order)} dot={order.status !== 'closed'}>
									{statusLabel(order)}
								</Badge>
							</div>

							{#each order.lines as line (line.lineId)}
								<div class="border-border flex flex-wrap items-center gap-3 border-t pt-3">
									<div class="min-w-0 flex-1">
										<p class="text-body truncate">{productName(line.productId)}</p>
										<p class="text-secondary text-text-muted">
											{format.quantity(line.receivedMilli)} of {format.quantity(line.orderedMilli)}
											received at {format.moneyPlain(line.unitCostMinor)} each
										</p>
									</div>

									{#if line.priceChanged}
										<!-- The entire reason to keep an order document. -->
										<Badge tone="warn">Price changed</Badge>
									{/if}

									{#if line.outstandingMilli > 0}
										<Badge tone="neutral">
											{format.quantity(line.outstandingMilli)} outstanding
										</Badge>
									{:else if line.outstandingMilli < 0}
										<Badge tone="warn">
											{format.quantity(-line.outstandingMilli)} over
										</Badge>
									{/if}

									{#if order.status !== 'closed'}
										<Button
											variant="secondary"
											size="xs"
											onclick={() => startReceive(order.id, line)}
											disabled={busy}
										>
											Receive
										</Button>
									{/if}
								</div>
							{/each}

							<div class="border-border flex items-center justify-between gap-3 border-t pt-3">
								<div class="flex gap-4">
									<span class="text-secondary text-text-secondary">
										Ordered <Numeric
											value={format.moneyPlain(order.orderedValueMinor)}
											class="text-secondary"
										/>
									</span>
									<span class="text-secondary text-text-secondary">
										Charged <Numeric
											value={format.moneyPlain(order.receivedValueMinor)}
											class="text-secondary"
										/>
									</span>
								</div>
								{#if order.status !== 'closed'}
									<Button
										variant="ghost"
										size="xs"
										onclick={() => {
											closing = order.id;
											closeReason =
												order.status === 'fully_received' ? 'complete' : 'short_shipped';
										}}
										disabled={busy}
									>
										Close
									</Button>
								{/if}
							</div>
						</div>
					</Card>
				{/each}
			</div>

			<div class="flex flex-col gap-4">
				{#if receiving}
					<Card label="Book the delivery in">
						<div class="flex flex-col gap-3">
							<p class="text-secondary text-text-secondary">
								Receiving <strong>{productName(receiving.line.productId)}</strong>. Enter what
								actually arrived and what was actually charged — a disagreement with the order is
								the thing worth catching.
							</p>

							<Field id="receive-quantity" label="Quantity received">
								{#snippet children({ id, describedBy })}
									<Input
										{id}
										{describedBy}
										bind:value={receiveQuantity}
										numeric
										forceLtr
										placeholder="30"
									/>
								{/snippet}
							</Field>

							<Field id="receive-cost" label="Unit cost charged">
								{#snippet children({ id, describedBy })}
									<Input
										{id}
										{describedBy}
										bind:value={receiveCost}
										numeric
										forceLtr
										placeholder="40.00"
									/>
								{/snippet}
							</Field>

							<Field id="receive-lot" label="Lot" hint="Optional. What a recall traces along.">
								{#snippet children({ id, describedBy })}
									<Input {id} {describedBy} bind:value={receiveLot} placeholder="KT-4471" />
								{/snippet}
							</Field>

							<Field
								id="receive-expiry"
								label="Expires"
								hint="Leave blank if it does not expire — YYYY-MM-DD."
							>
								{#snippet children({ id, describedBy })}
									<Input
										{id}
										{describedBy}
										bind:value={receiveExpiry}
										forceLtr
										placeholder="2026-12-31"
									/>
								{/snippet}
							</Field>

							<div class="flex gap-2">
								<Button variant="primary" size="lg" onclick={submitReceive} disabled={busy}>
									Book it in
								</Button>
								<Button
									variant="ghost"
									size="lg"
									onclick={() => (receiving = null)}
									disabled={busy}
								>
									Cancel
								</Button>
							</div>
						</div>
					</Card>
				{:else if closing}
					<Card label="Close the order">
						<div class="flex flex-col gap-3">
							<p class="text-secondary text-text-secondary">
								Closing settles what is expected. It does not pretend missing stock arrived.
							</p>
							<Field id="close-reason" label="Reason">
								{#snippet children({ id, describedBy })}
									<select
										{id}
										aria-describedby={describedBy}
										bind:value={closeReason}
										class="border-border bg-surface text-body w-full border px-3"
										style="min-height: var(--scale-touch-target)"
									>
										{#each CLOSE_REASONS as reason (reason.value)}
											<option value={reason.value}>{reason.label}</option>
										{/each}
									</select>
								{/snippet}
							</Field>
							<div class="flex gap-2">
								<Button variant="danger" size="lg" onclick={submitClose} disabled={busy}>
									Close order
								</Button>
								<Button variant="ghost" size="lg" onclick={() => (closing = null)} disabled={busy}>
									Cancel
								</Button>
							</div>
						</div>
					</Card>
				{:else}
					<Card label="Place an order">
						<div class="flex flex-col gap-3">
							<Field id="order-supplier" label="Supplier">
								{#snippet children({ id, describedBy })}
									<Input {id} {describedBy} bind:value={supplier} placeholder="Karim Traders" />
								{/snippet}
							</Field>

							<Field
								id="order-reference"
								label="Reference"
								hint="Optional. What the supplier knows it by."
							>
								{#snippet children({ id, describedBy })}
									<Input {id} {describedBy} bind:value={reference} placeholder="KT-4471" />
								{/snippet}
							</Field>

							<Field id="order-expected" label="Expected" hint="Optional — YYYY-MM-DD.">
								{#snippet children({ id, describedBy })}
									<Input
										{id}
										{describedBy}
										bind:value={expected}
										forceLtr
										placeholder="2026-08-05"
									/>
								{/snippet}
							</Field>

							{#each draftLines as line, index (index)}
								<div class="border-border flex flex-col gap-2 border-t pt-3">
									<Field id="line-product-{index}" label="Product">
										{#snippet children({ id, describedBy })}
											<select
												{id}
												aria-describedby={describedBy}
												bind:value={line.productId}
												class="border-border bg-surface text-body w-full border px-3"
												style="min-height: var(--scale-touch-target)"
											>
												{#each PRODUCTS as product (product.id)}
													<option value={product.id}>{product.name}</option>
												{/each}
											</select>
										{/snippet}
									</Field>
									<div class="flex gap-2">
										<Field id="line-quantity-{index}" label="Quantity" class="flex-1">
											{#snippet children({ id, describedBy })}
												<Input
													{id}
													{describedBy}
													bind:value={line.quantity}
													numeric
													forceLtr
													placeholder="50"
												/>
											{/snippet}
										</Field>
										<Field id="line-cost-{index}" label="Unit cost" class="flex-1">
											{#snippet children({ id, describedBy })}
												<Input
													{id}
													{describedBy}
													bind:value={line.unitCost}
													numeric
													forceLtr
													placeholder="40"
												/>
											{/snippet}
										</Field>
									</div>
									{#if draftLines.length > 1}
										<Button
											variant="ghost"
											size="xs"
											onclick={() => draftLines.splice(index, 1)}
											disabled={busy}
										>
											Remove line
										</Button>
									{/if}
								</div>
							{/each}

							<Button
								variant="ghost"
								size="lg"
								onclick={() =>
									draftLines.push({
										productId: PRODUCTS[0]?.id ?? '',
										quantity: '',
										unitCost: ''
									})}
								disabled={busy}
							>
								Add a line
							</Button>

							<Button variant="primary" size="lg" onclick={place} disabled={busy}>
								Place order
							</Button>
						</div>
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
