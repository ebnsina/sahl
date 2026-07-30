<script lang="ts">
	/**
	 * The shift close-out screen.
	 *
	 * `touch` density like the sell screen — this is used standing at a counter with a drawer open,
	 * often at the end of a long day, which is precisely when a mis-tap is most likely.
	 *
	 * The blind count is the point of this screen. A cashier who can see what the drawer *should*
	 * hold counts to that number rather than counting the cash, and the variance — the only signal
	 * that says anything about the day — becomes zero by construction. So the counting panel is fed
	 * by a separate command that never sends the expected figure at all. Hiding it in CSS or behind
	 * a conditional would leave it one devtools panel away.
	 */
	import {
		Badge,
		Button,
		Card,
		Field,
		Input,
		Numeric,
		Select,
		createFormatters,
		parseMinor
	} from '@sahl/ui';
	import PinPrompt from '$lib/PinPrompt.svelte';
	import { asTillError, isTillAvailable, till, type CashReason, type ShiftView } from '$lib/till';

	const format = createFormatters({ locale: 'en', currency: 'BDT', timeZone: 'Asia/Dhaka' });

	const REASONS: Array<{ value: CashReason; label: string; sign: -1 | 1 }> = [
		{ value: 'float_top_up', label: 'Add to float', sign: 1 },
		{ value: 'skim', label: 'Lift to safe', sign: -1 },
		{ value: 'petty_cash', label: 'Petty cash', sign: -1 },
		{ value: 'refund', label: 'Refund out', sign: -1 },
		{ value: 'correction', label: 'Correction', sign: -1 }
	];

	let shift = $state<ShiftView | null>(null);
	/** The figures with expectations withheld. Drives the counting panel and nothing else. */
	let sheet = $state<ShiftView | null>(null);
	let error = $state<{ code: string; message: string } | null>(null);
	let busy = $state(false);
	let available = $state(true);

	let floatInput = $state('');
	let countInput = $state('');
	let moveInput = $state('');
	let moveReason = $state<CashReason>('skim');
	let moveNote = $state('');
	/** True while the cashier is counting, before the variance is revealed. */
	let counting = $state(false);
	/** Set while a cash movement is waiting on a manager's PIN. */
	let pendingMove = $state<{ amountMinor: number; reason: CashReason; note: string | null } | null>(
		null
	);
	let approvalError = $state<string | null>(null);

	let counted = $derived(shift?.countedCashMinor !== null && shift?.countedCashMinor !== undefined);
	let closed = $derived(shift?.isFinal === true);

	async function run<T>(action: () => Promise<T>, onDone?: (result: T) => void) {
		busy = true;
		error = null;
		try {
			onDone?.(await action());
		} catch (thrown) {
			error = asTillError(thrown);
			if (error.code === 'no_till') available = false;
			// No open shift is the ordinary state of a till before the day starts, not a fault.
			if (error.code === 'no_open_shift') {
				shift = null;
				error = null;
			}
		} finally {
			busy = false;
		}
	}

	$effect(() => {
		available = isTillAvailable();
		if (available) {
			void run(
				() => till.shiftReport(),
				(result) => (shift = result)
			);
		}
	});

	function amount(entry: string, field: string): number | null {
		const minor = parseMinor(entry, 'BDT');
		if (minor === null) {
			error = { code: 'bad_amount', message: `Enter ${field} like 500 or 499.50` };
			return null;
		}
		return minor;
	}

	function openShift() {
		const minor = amount(floatInput, 'the opening float');
		if (minor === null) return;
		void run(
			() => till.openShift(minor),
			(result) => {
				shift = result;
				floatInput = '';
			}
		);
	}

	function startCount() {
		// Fetch the blind sheet before showing the panel, so the expected figure is never in this
		// component's state while the cashier is counting.
		void run(
			() => till.blindCountSheet(),
			(result) => {
				sheet = result;
				countInput = '';
				counting = true;
			}
		);
	}

	function submitCount() {
		const minor = amount(countInput, 'the counted cash');
		if (minor === null) return;
		void run(
			() => till.countDrawer(minor),
			(result) => {
				shift = result;
				sheet = null;
				counting = false;
				countInput = '';
			}
		);
	}

	function moveCash() {
		const minor = amount(moveInput, 'the amount');
		if (minor === null) return;
		if (minor <= 0) {
			error = {
				code: 'bad_amount',
				message: 'Enter a positive amount; the reason sets direction.'
			};
			return;
		}
		// Direction comes from the reason, not from the cashier typing a minus sign. A skim entered
		// as positive would silently inflate the drawer it was meant to reduce.
		const sign = REASONS.find((reason) => reason.value === moveReason)?.sign ?? -1;
		approvalError = null;
		pendingMove = {
			amountMinor: minor * sign,
			reason: moveReason,
			note: moveNote.trim() || null
		};
	}

	function confirmMove(pin: string) {
		const move = pendingMove;
		if (!move) return;
		void (async () => {
			busy = true;
			approvalError = null;
			try {
				shift = await till.moveCash({ ...move, pin });
				pendingMove = null;
				moveInput = '';
				moveNote = '';
			} catch (thrown) {
				const failure = asTillError(thrown);
				// A refused PIN keeps the prompt open — the manager mistyped and will try again.
				if (failure.code === 'not_authorized' || failure.code === 'no_approver') {
					approvalError = failure.message;
				} else {
					error = failure;
					pendingMove = null;
				}
			} finally {
				busy = false;
			}
		})();
	}

	function closeShift() {
		const current = shift;
		if (!current?.countedCashMinor) return;
		void run(
			() => till.closeShift(current.countedCashMinor ?? 0),
			(result) => (shift = result)
		);
	}

	function varianceTone(variance: ShiftView['variance']) {
		if (variance === 'balanced') return 'success' as const;
		return 'danger' as const;
	}
</script>

<svelte:head><title>Shift — Sahl</title></svelte:head>

<main class="bg-canvas text-text flex h-dvh flex-col" data-density="touch">
	<header class="border-border bg-surface flex items-center justify-between border-b px-4 py-3">
		<div class="flex items-center gap-3">
			<h1 class="text-lg font-semibold">Shift</h1>
			{#if shift}
				{#if closed}
					<Badge tone="neutral">Closed — Z report</Badge>
				{:else}
					<Badge tone="success" dot>Open — X report</Badge>
				{/if}
			{/if}
		</div>
		<div class="flex gap-4">
			<a href="/floor" class="text-secondary text-text-secondary hover:text-text underline">
				Floor
			</a>
			<a href="/catalogue" class="text-secondary text-text-secondary hover:text-text underline">
				Catalogue
			</a>
			<a href="/stock" class="text-secondary text-text-secondary hover:text-text underline">Stock</a
			>
			<a href="/orders" class="text-secondary text-text-secondary hover:text-text underline">
				Orders
			</a>
			<a href="/staff" class="text-secondary text-text-secondary hover:text-text underline">
				Staff
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
	{:else if !shift}
		<div class="flex flex-1 items-center justify-center p-8">
			<Card label="Open the till" class="w-full max-w-md">
				<div class="flex flex-col gap-4">
					<p class="text-secondary text-text-secondary">
						Count the starting float into the drawer and enter it. Everything the shift reports
						afterwards is measured from this number.
					</p>
					<Field id="opening-float" label="Opening float" hint="Counted, not assumed.">
						{#snippet children({ id, describedBy })}
							<Input
								{id}
								{describedBy}
								bind:value={floatInput}
								numeric
								forceLtr
								placeholder="2000"
							/>
						{/snippet}
					</Field>
					<Button variant="primary" size="lg" onclick={openShift} disabled={busy}>
						Open shift
					</Button>
				</div>
			</Card>
		</div>
	{:else}
		<div class="grid min-h-0 flex-1 grid-cols-1 gap-4 overflow-y-auto p-4 lg:grid-cols-[1fr_24rem]">
			<div class="flex flex-col gap-4">
				<Card label={closed ? 'Z report' : 'X report'}>
					<dl class="flex flex-col gap-2">
						{#snippet row(term: string, value: string, strong = false)}
							<div class="flex items-baseline justify-between">
								<dt class="text-body {strong ? 'font-semibold' : 'text-text-secondary'}">{term}</dt>
								<dd>
									<Numeric {value} class={strong ? 'text-md font-semibold' : ''} />
								</dd>
							</div>
						{/snippet}

						{@render row('Opening float', format.moneyPlain(shift.openingFloatMinor))}
						{@render row('Takings', format.moneyPlain(shift.takingsMinor))}
						{@render row('Cash from sales', format.moneyPlain(shift.cashFromSalesMinor))}
						{@render row('Cash in and out', format.moneyPlain(shift.netMovementsMinor))}

						<div class="border-border mt-1 border-t pt-2">
							{@render row('Expected in drawer', format.money(shift.expectedCashMinor), true)}
						</div>

						{#if counted}
							{@render row('Counted', format.money(shift.countedCashMinor ?? 0), true)}
						{/if}
					</dl>
				</Card>

				{#if counted && shift.variance}
					<Card label="Variance">
						<div class="flex items-center justify-between gap-4">
							<Badge tone={varianceTone(shift.variance)} dot>
								{shift.variance === 'balanced'
									? 'Balanced'
									: shift.variance === 'short'
										? 'Short'
										: 'Over'}
							</Badge>
							{#if shift.variance !== 'balanced'}
								<Numeric
									value={format.money(shift.varianceMinor ?? 0)}
									class="text-lg font-semibold"
								/>
							{/if}
						</div>
						{#if shift.variance === 'over'}
							<p class="text-secondary text-text-secondary mt-2">
								Over is not automatically good news — a drawer that is consistently over usually
								means sales are going unrecorded and the cash is arriving anyway.
							</p>
						{/if}
					</Card>
				{/if}

				<Card label="Activity">
					<div class="grid grid-cols-3 gap-3">
						<div>
							<p class="label-caps">Sales</p>
							<Numeric value={format.integer(shift.saleCount)} align="start" class="text-md" />
						</div>
						<div>
							<p class="label-caps">Voids</p>
							<Numeric value={format.integer(shift.voidCount)} align="start" class="text-md" />
						</div>
						<div>
							<p class="label-caps">Counts</p>
							<Numeric value={format.integer(shift.countAttempts)} align="start" class="text-md" />
						</div>
					</div>
					{#if shift.countAttempts > 1}
						<p class="text-secondary text-text-secondary mt-2">
							Counted more than once. Every attempt is kept — a recount that lands on the expected
							figure is worth reading alongside the one before it.
						</p>
					{/if}
				</Card>
			</div>

			<div class="flex flex-col gap-4">
				{#if !closed}
					<Card label="Count the drawer">
						{#if counting && sheet}
							<div class="flex flex-col gap-4">
								<p class="text-secondary text-text-secondary">
									Count the cash in the drawer and enter the total. The expected figure is
									deliberately not shown — counting towards a number is not counting.
								</p>
								<Field id="counted-cash" label="Counted cash">
									{#snippet children({ id, describedBy })}
										<Input
											{id}
											{describedBy}
											bind:value={countInput}
											numeric
											forceLtr
											placeholder="0.00"
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
										}}
										disabled={busy}>Cancel</Button
									>
								</div>
							</div>
						{:else}
							<div class="flex flex-col gap-3">
								<p class="text-secondary text-text-secondary">
									{counted
										? 'Counted. Count again if the drawer was recounted.'
										: 'A blind count — you will not see the expected figure while counting.'}
								</p>
								<Button variant="primary" size="lg" onclick={startCount} disabled={busy}>
									{counted ? 'Count again' : 'Start count'}
								</Button>
							</div>
						{/if}
					</Card>

					<Card label="Cash in and out">
						<div class="flex flex-col gap-3">
							<Field id="move-reason" label="Reason">
								{#snippet children({ id, describedBy })}
									<Select {id} {describedBy} bind:value={moveReason} options={REASONS} />
								{/snippet}
							</Field>

							<Field
								id="move-amount"
								label="Amount"
								hint="Always positive — the reason sets the direction."
							>
								{#snippet children({ id, describedBy })}
									<Input
										{id}
										{describedBy}
										bind:value={moveInput}
										numeric
										forceLtr
										placeholder="1000"
									/>
								{/snippet}
							</Field>

							<Field
								id="move-note"
								label="Note"
								hint="Optional, but a correction without one explains nothing."
							>
								{#snippet children({ id, describedBy })}
									<Input {id} {describedBy} bind:value={moveNote} placeholder="Drop to safe" />
								{/snippet}
							</Field>

							<Button variant="secondary" size="lg" onclick={moveCash} disabled={busy}>
								Record movement
							</Button>
						</div>
					</Card>

					<Card label="Close the till">
						<div class="flex flex-col gap-3">
							<p class="text-secondary text-text-secondary">
								{counted
									? 'Nothing can be added to this shift once it is closed.'
									: 'Count the drawer before closing — the till refuses a close without one.'}
							</p>
							<Button variant="danger" size="lg" onclick={closeShift} disabled={busy || !counted}>
								Close shift
							</Button>
						</div>
					</Card>
				{:else}
					<Card label="Closed">
						<p class="text-secondary text-text-secondary">
							This shift is settled. Open a new one to keep selling.
						</p>
					</Card>
				{/if}
			</div>
		</div>
	{/if}

	{#if pendingMove}
		<PinPrompt
			action="Move cash in or out of the drawer"
			{busy}
			error={approvalError}
			onsubmit={confirmMove}
			oncancel={() => {
				pendingMove = null;
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
