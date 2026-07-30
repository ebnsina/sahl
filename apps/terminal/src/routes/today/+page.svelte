<script lang="ts">
	/**
	 * What the day came to.
	 *
	 * Every figure here is computed in Rust by `sahl_core::report` — the same crate the till sells
	 * with. Nothing on this page adds anything up. A screen that totalled its own rows would be a
	 * second implementation of the money rules, and the first day it disagreed with the shift
	 * report nobody would know which one to believe.
	 *
	 * `compact` density: this is read, not tapped.
	 */
	import { Card, Numeric } from '@sahl/ui';
	import { loadShop, shop } from '$lib/outlet.svelte';
	import { asTillError, isTillAvailable, till, type DayView, type FindingView } from '$lib/till';

	const format = $derived(shop.formatters);

	let day = $state<DayView | null>(null);
	let findings = $state<FindingView[]>([]);
	let error = $state<{ code: string; message: string } | null>(null);
	let available = $state(true);

	$effect(() => {
		available = isTillAvailable();
		if (!available) return;
		void loadShop();
		void (async () => {
			try {
				day = await till.dayReport();
				findings = await till.anomalyFeed();
			} catch (thrown) {
				error = asTillError(thrown);
				if (error.code === 'no_till') available = false;
			}
		})();
	});

	/** The share of takings a row accounts for, for the bar width. */
	function share(part: number, whole: number): number {
		if (whole <= 0) return 0;
		return Math.min(100, Math.round((part / whole) * 100));
	}
</script>

<svelte:head><title>Today · Sahl</title></svelte:head>

<main class="bg-canvas text-text density-compact min-h-dvh">
	<header
		class="border-border bg-surface flex items-center justify-between gap-4 border-b px-4 py-3"
	>
		<h1 class="text-lg font-semibold">Today</h1>
		<div class="flex items-center gap-4">
			<a href="/" class="text-secondary text-text-secondary hover:text-text underline"
				>Sell screen</a
			>
			<a href="/shift" class="text-secondary text-text-secondary hover:text-text underline">Shift</a
			>
			<a href="/staff" class="text-secondary text-text-secondary hover:text-text underline">Staff</a
			>
			<a href="/settings" class="text-secondary text-text-secondary hover:text-text underline">
				Settings
			</a>
		</div>
	</header>

	{#if !available}
		<div class="flex items-center justify-center p-8">
			<Card label="Not connected" class="max-w-lg">
				<p class="text-md">This screen runs inside the Sahl till application.</p>
				<p class="text-secondary text-text-secondary mt-2">
					Every figure comes from the till, which owns the arithmetic. A browser stand-in would mean
					a second implementation of the money rules.
				</p>
			</Card>
		</div>
	{:else if day}
		<div class="grid grid-cols-1 gap-4 p-4 lg:grid-cols-[1fr_22rem]">
			<div class="flex flex-col gap-4">
				<Card label="Takings">
					<div class="grid grid-cols-2 gap-4 sm:grid-cols-4">
						<div>
							<p class="label-caps">Taken</p>
							<Numeric value={format.money(day.takingsMinor)} class="text-xl font-semibold" />
						</div>
						<div>
							<p class="label-caps">Sales</p>
							<Numeric value={format.integer(day.sales)} class="text-xl" />
						</div>
						<div>
							<p class="label-caps">Average sale</p>
							<Numeric value={format.money(day.averageSaleMinor)} class="text-xl" />
						</div>
						<div>
							<p class="label-caps">VAT</p>
							<Numeric value={format.money(day.taxMinor)} class="text-xl" />
						</div>
					</div>

					<div class="border-border mt-4 grid grid-cols-2 gap-4 border-t pt-3 sm:grid-cols-4">
						<div>
							<p class="label-caps">Excluding VAT</p>
							<Numeric value={format.moneyPlain(day.netMinor)} />
						</div>
						<div>
							<p class="label-caps">Discount given</p>
							<Numeric value={format.moneyPlain(day.discountMinor)} />
						</div>
						<div>
							<p class="label-caps">Lines voided</p>
							<Numeric value={format.integer(day.voids)} />
						</div>
					</div>
				</Card>

				<Card label="What sold" flush>
					{#if day.byProduct.length === 0}
						<p class="text-secondary text-text-muted p-4">Nothing yet today.</p>
					{:else}
						{#each day.byProduct as row (row.name)}
							<div class="border-border flex items-center gap-3 border-b px-3 py-2">
								<div class="min-w-0 flex-1">
									<p class="text-body truncate">{row.name}</p>
									<!-- Width from the share of takings, so the eye ranks them before the
									     numbers are read. -->
									<div class="bg-surface-sunken mt-1 h-1 w-full">
										<div
											class="bg-primary h-1"
											style="width: {share(row.revenueMinor, day.takingsMinor)}%"
										></div>
									</div>
								</div>
								<Numeric value={format.quantity(row.quantityMilli)} class="text-text-secondary" />
								<Numeric value={format.moneyPlain(row.revenueMinor)} />
							</div>
						{/each}
					{/if}
				</Card>
			</div>

			<div class="flex flex-col gap-4">
				<Card label="How they paid" flush>
					{#if day.byPayment.length === 0}
						<p class="text-secondary text-text-muted p-4">Nothing taken yet.</p>
					{:else}
						{#each day.byPayment as row (row.method)}
							<div class="border-border flex items-center gap-3 border-b px-3 py-2">
								<span class="text-body min-w-0 flex-1 truncate">{row.method}</span>
								<span class="text-secondary text-text-muted">
									{format.integer(row.count)}
								</span>
								<Numeric value={format.moneyPlain(row.takenMinor)} />
							</div>
						{/each}
					{/if}
				</Card>

				<Card label="Who rang it" flush>
					{#if day.byCashier.length === 0}
						<p class="text-secondary text-text-muted p-4">Nobody has rung anything yet.</p>
					{:else}
						{#each day.byCashier as row (row.name)}
							<div class="border-border flex items-center gap-3 border-b px-3 py-2">
								<div class="min-w-0 flex-1">
									<p class="text-body truncate">{row.name}</p>
									<p class="text-secondary text-text-muted">
										{format.integer(row.sales)} sales · {format.integer(row.voids)} voided
									</p>
								</div>
								<Numeric value={format.moneyPlain(row.takingsMinor)} />
							</div>
						{/each}
					{/if}
				</Card>

				{#if findings.length > 0}
					<Card label="Worth a look" flush>
						<p class="text-secondary text-text-muted border-border border-b p-3">
							Questions to ask, not conclusions. Whoever works the returns counter will always void
							more.
						</p>
						{#each findings as finding (finding.kind + (finding.person ?? ''))}
							<div class="border-border border-b px-3 py-2">
								<p class="text-body">{finding.summary}</p>
								{#if finding.person}
									<p class="text-secondary text-text-muted">{finding.person}</p>
								{/if}
							</div>
						{/each}
					</Card>
				{/if}
			</div>
		</div>
	{/if}

	{#if error}
		<div class="border-danger bg-danger-subtle text-danger-text border-t px-4 py-3">
			<p class="text-body">{error.message}</p>
		</div>
	{/if}
</main>
