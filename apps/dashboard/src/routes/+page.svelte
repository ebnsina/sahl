<script lang="ts">
	/**
	 * What the shop did.
	 *
	 * Phone-first: this is read standing somewhere else, which is the entire promise — the owner
	 * always knows. Every figure arrives totalled by `sahl_core::report`; nothing here adds
	 * anything up, for the same reason the terminal's Today screen does not.
	 */
	import { Card, Numeric, Select, createFormatters, type CurrencyCode } from '@sahl/ui';

	let { data } = $props();

	// The outlet's own currency, from the figures themselves. A dashboard that assumed taka would
	// put a taka sign over a Riyadh café's riyals — which is exactly what the terminal used to do.
	const format = $derived(
		createFormatters({
			locale: 'en',
			currency: (data.today?.currency ?? 'BDT') as CurrencyCode,
			timeZone: 'UTC'
		})
	);

	function share(part: number, whole: number): number {
		if (whole <= 0) return 0;
		return Math.min(100, Math.round((part / whole) * 100));
	}
</script>

<svelte:head><title>Today · Sahl</title></svelte:head>

<main class="bg-canvas text-text density-compact min-h-dvh">
	<header
		class="border-border bg-surface flex flex-wrap items-center justify-between gap-3 border-b px-4 py-3"
	>
		<h1 class="text-lg font-semibold">Today</h1>

		{#if data.shops.length > 1}
			<form method="GET" class="flex items-center gap-2">
				<Select
					id="outlet"
					value={data.chosen ?? ''}
					options={data.shops.map((shop) => ({ value: shop.id, label: shop.name }))}
					onchange={(event) => (event.currentTarget as HTMLSelectElement).form?.submit()}
					class="min-w-48"
				/>
				<noscript><button type="submit">Show</button></noscript>
			</form>
		{:else if data.shops[0]}
			<span class="text-secondary text-text-secondary">{data.shops[0].name}</span>
		{/if}
	</header>

	{#if data.error}
		<div class="border-danger bg-danger-subtle text-danger-text m-4 border p-3">
			<p class="text-body">{data.error}</p>
		</div>
	{:else if data.today}
		{@const today = data.today}
		<div class="flex flex-col gap-4 p-4">
			<Card label="Takings">
				<div class="grid grid-cols-2 gap-4 sm:grid-cols-4">
					<div>
						<p class="label-caps">Taken</p>
						<Numeric value={format.money(today.takings.minor)} class="text-xl font-semibold" />
					</div>
					<div>
						<p class="label-caps">Sales</p>
						<Numeric value={format.integer(today.sales)} class="text-xl" />
					</div>
					<div>
						<p class="label-caps">Average sale</p>
						<Numeric value={format.money(today.average_sale.minor)} class="text-xl" />
					</div>
					<div>
						<p class="label-caps">VAT</p>
						<Numeric value={format.money(today.tax.minor)} class="text-xl" />
					</div>
				</div>

				<div class="border-border mt-4 grid grid-cols-2 gap-4 border-t pt-3 sm:grid-cols-4">
					<div>
						<p class="label-caps">Excluding VAT</p>
						<Numeric value={format.moneyPlain(today.net.minor)} />
					</div>
					<div>
						<p class="label-caps">Discount given</p>
						<Numeric value={format.moneyPlain(today.discount.minor)} />
					</div>
					<div>
						<p class="label-caps">Lines voided</p>
						<Numeric value={format.integer(today.voids)} />
					</div>
				</div>
			</Card>

			<div class="grid grid-cols-1 gap-4 lg:grid-cols-2">
				<Card label="What sold" flush>
					{#if today.by_product.length === 0}
						<p class="text-secondary text-text-muted p-4">Nothing yet.</p>
					{:else}
						{#each today.by_product as row (row.product_id)}
							<div class="border-border flex items-center gap-3 border-b px-3 py-2">
								<div class="min-w-0 flex-1">
									<p class="text-body truncate">{row.name}</p>
									<div class="bg-surface-sunken mt-1 h-1 w-full">
										<div
											class="bg-primary h-1"
											style="width: {share(row.revenue.minor, today.takings.minor)}%"
										></div>
									</div>
								</div>
								<Numeric value={format.quantity(row.quantity_milli)} class="text-text-secondary" />
								<Numeric value={format.moneyPlain(row.revenue.minor)} />
							</div>
						{/each}
					{/if}
				</Card>

				<Card label="Who rang it" flush>
					{#if today.by_cashier.length === 0}
						<p class="text-secondary text-text-muted p-4">Nobody has rung anything yet.</p>
					{:else}
						{#each today.by_cashier as row (row.staff_id)}
							<div class="border-border flex items-center gap-3 border-b px-3 py-2">
								<div class="min-w-0 flex-1">
									<!-- Ids, not names: staff live in the event log, and the server does not yet
									     project the directory. Better a number the owner can match against the
									     till than a name this build guessed at. -->
									<p class="text-body truncate">
										<span class="numeric">{row.staff_id.slice(0, 8)}</span>
									</p>
									<p class="text-secondary text-text-muted">
										{format.integer(row.sales)} sales · {format.integer(row.voids)} voided
									</p>
								</div>
								<Numeric value={format.moneyPlain(row.takings.minor)} />
							</div>
						{/each}
					{/if}
				</Card>
			</div>
		</div>
	{:else}
		<p class="text-secondary text-text-muted p-4">
			No outlet has synced anything yet. A till starts sending as soon as it has a server.
		</p>
	{/if}
</main>
