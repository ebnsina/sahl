<script lang="ts">
	/**
	 * The floor plan — a café's working surface.
	 *
	 * Occupancy is derived from the open tickets rather than stored on a table. That is why a table
	 * frees itself the moment its ticket settles, with no extra step for anyone to forget: a table
	 * holding its own ticket id would need keeping in step with the sale, and the two disagreeing is
	 * how a café ends up unable to seat a table it can see is empty.
	 *
	 * Only shown for a café. The capability comes from the outlet's profile, which is a row rather
	 * than a branch — a retail outlet simply has no tables and never sees this.
	 */
	import { Badge, Button, Card, Field, Input, createFormatters } from '@sahl/ui';
	import PinPrompt from '$lib/PinPrompt.svelte';
	import { asTillError, isTillAvailable, till, type OutletView, type TableView } from '$lib/till';

	const format = createFormatters({ locale: 'en', currency: 'BDT', timeZone: 'Asia/Dhaka' });

	let tables = $state<TableView[]>([]);
	let outlet = $state<OutletView | null>(null);
	let error = $state<{ code: string; message: string } | null>(null);
	let busy = $state(false);
	let available = $state(true);

	let editing = $state<TableView | null>(null);
	let showForm = $state(false);
	let label = $state('');
	let section = $state('');
	let seats = $state('4');

	/** The table being seated, and how many people. */
	let seating = $state<TableView | null>(null);
	let covers = $state('2');

	let pendingSave = $state(false);
	let pendingToggle = $state<TableView | null>(null);
	let approvalError = $state<string | null>(null);

	let isCafe = $derived(outlet?.capabilities.includes('table_service') ?? false);
	let inService = $derived(tables.filter((table) => table.active));
	let occupied = $derived(inService.filter((table) => table.saleId !== null));
	let sections = $derived([...new Set(inService.map((table) => table.section ?? 'Unsectioned'))]);

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
			void run(async () => {
				tables = await till.floorPlan(true);
				outlet = await till.outletConfig();
			});
		}
	});

	function startAdd() {
		editing = null;
		label = '';
		section = sections[0] === 'Unsectioned' ? '' : (sections[0] ?? '');
		seats = '4';
		showForm = true;
	}

	function startEdit(table: TableView) {
		editing = table;
		label = table.label;
		section = table.section ?? '';
		seats = String(table.seats);
		showForm = true;
	}

	function startSave() {
		if (!label.trim()) {
			error = { code: 'bad_label', message: 'Give the table a label — "4", "T12", "Bar 3"' };
			return;
		}
		const count = Number(seats);
		if (!Number.isInteger(count) || count < 1 || count > 30) {
			error = { code: 'bad_seats', message: 'Seats must be between 1 and 30' };
			return;
		}
		approvalError = null;
		pendingSave = true;
	}

	function confirmSave(pin: string) {
		void (async () => {
			busy = true;
			approvalError = null;
			try {
				tables = await till.saveTable({
					tableId: editing?.id ?? null,
					label,
					section: section.trim() || null,
					seats: Number(seats),
					pin
				});
				pendingSave = false;
				showForm = false;
			} catch (thrown) {
				const failure = asTillError(thrown);
				if (failure.code === 'not_authorized' || failure.code === 'no_approver') {
					approvalError = failure.message;
				} else {
					error = failure;
					pendingSave = false;
				}
			} finally {
				busy = false;
			}
		})();
	}

	function confirmToggle(pin: string) {
		const target = pendingToggle;
		if (!target) return;
		void (async () => {
			busy = true;
			approvalError = null;
			try {
				tables = await till.setTableActive(target.id, !target.active, pin);
				pendingToggle = null;
			} catch (thrown) {
				const failure = asTillError(thrown);
				if (failure.code === 'not_authorized' || failure.code === 'no_approver') {
					approvalError = failure.message;
				} else {
					error = failure;
					pendingToggle = null;
				}
			} finally {
				busy = false;
			}
		})();
	}

	/** Open a ticket and sit it at this table, in one gesture. */
	function seatTable() {
		const table = seating;
		if (!table) return;
		const count = Number(covers);
		if (!Number.isInteger(count) || count < 1) {
			error = { code: 'bad_covers', message: 'How many people are sitting down?' };
			return;
		}

		void run(
			async () => {
				// A table is seated by opening a ticket for it. Nothing else in the product opens a
				// ticket without a first item, which is exactly the café difference.
				const sale = await till.openSale();
				await till.seatSale(sale.id, table.id, count);
				return till.floorPlan(true);
			},
			(result) => {
				tables = result;
				seating = null;
			}
		);
	}
</script>

<svelte:head><title>Floor — Sahl</title></svelte:head>

<main class="bg-canvas text-text flex h-dvh flex-col" data-density="touch">
	<header class="border-border bg-surface flex items-center justify-between border-b px-4 py-3">
		<div class="flex items-center gap-3">
			<h1 class="text-lg font-semibold">Floor</h1>
			{#if inService.length > 0}
				<Badge tone={occupied.length > 0 ? 'primary' : 'neutral'}>
					{format.integer(occupied.length)} of {format.integer(inService.length)} in use
				</Badge>
			{/if}
		</div>
		<div class="flex gap-4">
			<a href="/catalogue" class="text-secondary text-text-secondary hover:text-text underline">
				Catalogue
			</a>
			<a href="/settings" class="text-secondary text-text-secondary hover:text-text underline">
				Settings
			</a>
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
	{:else if !isCafe}
		<div class="flex flex-1 items-center justify-center p-8">
			<Card label="Not a café" class="max-w-lg">
				<div class="flex flex-col gap-3">
					<p class="text-md">This outlet does not use table service.</p>
					<p class="text-secondary text-text-secondary">
						Tables belong to the café profile. Change the profile in Settings and this becomes the
						working surface — the capability follows the profile rather than being a switch of its
						own, so two café outlets always behave the same way.
					</p>
					<a href="/settings" class="text-secondary text-primary-text underline">Open Settings</a>
				</div>
			</Card>
		</div>
	{:else}
		<div class="grid min-h-0 flex-1 grid-cols-1 gap-4 overflow-y-auto p-4 lg:grid-cols-[1fr_22rem]">
			<div class="flex min-h-0 flex-col gap-4">
				{#if inService.length === 0}
					<Card label="No tables">
						<p class="text-secondary text-text-muted">
							Add the room's tables and they appear here. Until then a café works exactly like
							retail — a ticket that opens and closes at the counter.
						</p>
					</Card>
				{/if}

				{#each sections as name (name)}
					<Card label={name}>
						<div class="grid grid-cols-2 gap-3 sm:grid-cols-3 xl:grid-cols-4">
							{#each inService.filter((table) => (table.section ?? 'Unsectioned') === name) as table (table.id)}
								<button
									type="button"
									disabled={busy}
									onclick={() => {
										if (table.saleId) {
											// An occupied table goes straight to its ticket. That is the gesture a
											// waiter makes a hundred times a service.
											location.href = '/';
										} else {
											covers = String(Math.min(table.seats, 2));
											seating = table;
										}
									}}
									class="flex flex-col items-start gap-1 border p-3 text-start transition-colors
									       disabled:cursor-not-allowed disabled:opacity-50
									       {table.saleId
										? 'border-primary bg-primary-subtle hover:brightness-95'
										: 'border-border bg-surface hover:bg-surface-hover'}"
									style="min-height: var(--scale-touch-target)"
								>
									<span class="text-md font-semibold">{table.label}</span>

									{#if table.saleId}
										<span class="numeric text-body">
											{format.moneyPlain(table.runningTotalMinor ?? 0)}
										</span>
										<span class="text-secondary text-text-secondary">
											{format.integer(table.covers ?? 0)} covers
										</span>
									{:else}
										<span class="text-secondary text-text-muted">
											Seats {format.integer(table.seats)}
										</span>
									{/if}
								</button>
							{/each}
						</div>
					</Card>
				{/each}
			</div>

			<div class="flex flex-col gap-4">
				{#if seating}
					<Card label="Seat table {seating.label}">
						<div class="flex flex-col gap-3">
							<Field
								id="covers"
								label="How many people"
								hint="The denominator of every per-head figure. A two-seat table often holds three."
							>
								{#snippet children({ id, describedBy })}
									<Input {id} {describedBy} bind:value={covers} numeric forceLtr placeholder="2" />
								{/snippet}
							</Field>
							<div class="flex gap-2">
								<Button variant="primary" size="lg" onclick={seatTable} disabled={busy}>
									Open a ticket
								</Button>
								<Button variant="ghost" size="lg" onclick={() => (seating = null)} disabled={busy}>
									Cancel
								</Button>
							</div>
						</div>
					</Card>
				{:else if showForm}
					<Card label={editing ? 'Edit table' : 'New table'}>
						<div class="flex flex-col gap-3">
							<Field id="table-label" label="Label" hint="What staff call it. Letters are fine.">
								{#snippet children({ id, describedBy })}
									<Input {id} {describedBy} bind:value={label} placeholder="4" />
								{/snippet}
							</Field>

							<Field
								id="table-section"
								label="Section"
								hint="Optional. Two sections may each have a table 1."
							>
								{#snippet children({ id, describedBy })}
									<Input {id} {describedBy} bind:value={section} placeholder="Terrace" />
								{/snippet}
							</Field>

							<Field id="table-seats" label="Seats">
								{#snippet children({ id, describedBy })}
									<Input {id} {describedBy} bind:value={seats} numeric forceLtr placeholder="4" />
								{/snippet}
							</Field>

							<div class="flex gap-2">
								<Button variant="primary" size="lg" onclick={startSave} disabled={busy}>
									{editing ? 'Save' : 'Add table'}
								</Button>
								<Button
									variant="ghost"
									size="lg"
									onclick={() => (showForm = false)}
									disabled={busy}
								>
									Cancel
								</Button>
							</div>
						</div>
					</Card>
				{:else}
					<Card label="The room">
						<div class="flex flex-col gap-3">
							<p class="text-secondary text-text-secondary">
								Tap an empty table to open a ticket on it. Tap an occupied one to go to its ticket.
								A table frees itself when its ticket settles — there is no separate step to forget.
							</p>
							<Button variant="primary" size="lg" onclick={startAdd} disabled={busy}>
								Add a table
							</Button>
						</div>
					</Card>
				{/if}

				<Card label="All tables" flush>
					{#each tables as table (table.id)}
						<div
							class="border-border flex items-center gap-2 border-b px-3"
							style="min-height: var(--scale-row-height)"
						>
							<div class="min-w-0 flex-1">
								<p class="text-body truncate {table.active ? '' : 'text-text-muted'}">
									{table.label}
								</p>
								<p class="text-secondary text-text-muted truncate">
									{table.section ?? 'No section'} · seats {format.integer(table.seats)}
								</p>
							</div>
							{#if !table.active}
								<Badge tone="neutral" dot>Out of service</Badge>
							{:else if table.saleId}
								<Badge tone="primary" dot>In use</Badge>
							{/if}
							<Button variant="ghost" size="xs" onclick={() => startEdit(table)} disabled={busy}>
								Edit
							</Button>
							<Button
								variant="ghost"
								size="xs"
								onclick={() => {
									approvalError = null;
									pendingToggle = table;
								}}
								disabled={busy}
							>
								{table.active ? 'Remove' : 'Restore'}
							</Button>
						</div>
					{/each}
				</Card>
			</div>
		</div>
	{/if}

	{#if pendingSave}
		<PinPrompt
			action={editing ? `Change table ${editing.label}` : 'Add a table to the floor'}
			{busy}
			error={approvalError}
			onsubmit={confirmSave}
			oncancel={() => {
				pendingSave = false;
				approvalError = null;
			}}
		/>
	{/if}

	{#if pendingToggle}
		<PinPrompt
			action={pendingToggle.active
				? `Take table ${pendingToggle.label} out of service`
				: `Put table ${pendingToggle.label} back in service`}
			{busy}
			error={approvalError}
			onsubmit={confirmToggle}
			oncancel={() => {
				pendingToggle = null;
				approvalError = null;
			}}
		/>
	{/if}

	{#if error}
		<div class="border-danger bg-danger-subtle text-danger-text border-t px-4 py-3">
			<p class="text-body">{error.message}</p>
		</div>
	{/if}
</main>
