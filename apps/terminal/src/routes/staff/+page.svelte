<script lang="ts">
	/**
	 * Staff and the audit feed.
	 *
	 * Two halves of one control on one screen, deliberately. The list decides who may approve what;
	 * the feed below it shows what they actually did. Reading either alone is how a shop ends up
	 * with a tidy permission table and no idea whether it is being followed.
	 *
	 * The first person enrolled needs no approval — there is nobody yet to give it. Everyone after
	 * them needs an owner's PIN, and the till enforces that, not this screen.
	 */
	import { Badge, Button, Card, Field, Input, Numeric, createFormatters } from '@sahl/ui';
	import PinPrompt from '$lib/PinPrompt.svelte';
	import { asTillError, isTillAvailable, till, type AuditView, type StaffView } from '$lib/till';

	const format = createFormatters({ locale: 'en', currency: 'BDT', timeZone: 'Asia/Dhaka' });

	const ROLES: Array<{ value: StaffView['role']; label: string; note: string }> = [
		{
			value: 'cashier',
			label: 'Cashier',
			note: 'Sells and counts. Cannot void, discount, or move cash.'
		},
		{
			value: 'manager',
			label: 'Manager',
			note: 'Runs the floor. Approves voids, discounts, cash movements.'
		},
		{ value: 'owner', label: 'Owner', note: 'Everything, including staff and devices.' }
	];

	let staff = $state<StaffView[]>([]);
	let feed = $state<AuditView[]>([]);
	let error = $state<{ code: string; message: string } | null>(null);
	let busy = $state(false);
	let available = $state(true);

	let name = $state('');
	let role = $state<StaffView['role']>('cashier');
	let newPin = $state('');
	/** Set while an enrolment is waiting on an owner's PIN. */
	let pendingEnrol = $state(false);
	let approvalError = $state<string | null>(null);

	let firstPerson = $derived(staff.length === 0);
	let flagged = $derived(feed.filter((entry) => entry.unapproved));
	let hasApprover = $derived(staff.some((member) => member.role !== 'cashier'));

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

	async function refresh() {
		staff = await till.staffList();
		feed = await till.auditFeed();
		// Keeps the disabled select honest: it shows owner because owner is what will be sent.
		if (staff.length === 0) role = 'owner';
	}

	$effect(() => {
		available = isTillAvailable();
		if (available) void run(refresh);
	});

	function startEnrol() {
		if (!name.trim()) {
			error = { code: 'bad_name', message: 'Enter a name' };
			return;
		}
		if (!/^\d{4,8}$/.test(newPin)) {
			error = { code: 'bad_pin', message: 'A PIN is 4 to 8 digits' };
			return;
		}
		// The very first person cannot be approved by anyone — there is nobody yet. The till applies
		// the same rule; this only decides whether to bother asking.
		if (firstPerson) {
			void run(
				() => till.enrolStaff({ name, role, newPin, pin: '' }),
				(result) => {
					staff = result;
					name = '';
					newPin = '';
				}
			);
			return;
		}
		approvalError = null;
		pendingEnrol = true;
	}

	function confirmEnrol(pin: string) {
		void (async () => {
			busy = true;
			approvalError = null;
			try {
				staff = await till.enrolStaff({ name, role, newPin, pin });
				feed = await till.auditFeed();
				pendingEnrol = false;
				name = '';
				newPin = '';
			} catch (thrown) {
				const failure = asTillError(thrown);
				if (failure.code === 'not_authorized' || failure.code === 'no_approver') {
					approvalError = failure.message;
				} else {
					error = failure;
					pendingEnrol = false;
				}
			} finally {
				busy = false;
			}
		})();
	}

	function roleTone(value: StaffView['role']): 'primary' | 'warn' | 'neutral' {
		if (value === 'owner') return 'warn';
		return value === 'manager' ? 'primary' : 'neutral';
	}

	function severityTone(severity: AuditView['severity']): 'danger' | 'warn' | 'neutral' {
		if (severity === 'alert') return 'danger';
		return severity === 'notable' ? 'warn' : 'neutral';
	}
</script>

<svelte:head><title>Staff — Sahl</title></svelte:head>

<main class="bg-canvas text-text flex min-h-dvh flex-col" data-density="touch">
	<header class="border-border bg-surface flex items-center justify-between border-b px-4 py-3">
		<div class="flex items-center gap-3">
			<h1 class="text-lg font-semibold">Staff</h1>
			{#if flagged.length > 0}
				<Badge tone="danger" dot>{format.integer(flagged.length)} unapproved</Badge>
			{/if}
			{#if available && staff.length > 0 && !hasApprover}
				<!-- Every approval path refuses in this state, which looks like a bug from the counter. -->
				<Badge tone="warn">No manager — nothing can be approved</Badge>
			{/if}
		</div>
		<div class="flex gap-4">
			<a href="/stock" class="text-secondary text-text-secondary hover:text-text underline">Stock</a
			>
			<a href="/orders" class="text-secondary text-text-secondary hover:text-text underline">
				Orders
			</a>
			<a href="/shift" class="text-secondary text-text-secondary hover:text-text underline">Shift</a
			>
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
	{:else}
		<div class="grid flex-1 grid-cols-1 gap-4 overflow-y-auto p-4 lg:grid-cols-[1fr_24rem]">
			<div class="flex flex-col gap-4">
				<Card label="Who works here" flush>
					{#if staff.length === 0}
						<p class="text-secondary text-text-muted p-4">
							Nobody enrolled yet. Until someone is, no void, discount or cash movement can be
							approved — the till will say so rather than letting them through.
						</p>
					{:else}
						{#each staff as member (member.id)}
							<div
								class="border-border flex items-center gap-3 border-b px-3"
								style="min-height: var(--scale-row-height)"
							>
								<div class="min-w-0 flex-1">
									<p class="text-body truncate">{member.name}</p>
								</div>
								<Badge tone={roleTone(member.role)}>
									{ROLES.find((entry) => entry.value === member.role)?.label ?? member.role}
								</Badge>
							</div>
						{/each}
					{/if}
				</Card>

				<Card label="What happened" flush>
					{#if feed.length === 0}
						<p class="text-secondary text-text-muted p-4">
							Nothing yet. This shows only what moved money without selling something — voids,
							discounts, cash in and out, drawer counts. Ordinary selling would drown it.
						</p>
					{:else}
						{#each feed as entry (entry.at + entry.kind + entry.actor)}
							<div class="border-border flex flex-wrap items-center gap-3 border-b px-3 py-2">
								<div class="min-w-0 flex-1">
									<p class="text-body truncate">{entry.summary}</p>
									<p class="text-secondary text-text-muted">
										{entry.actorName}
										{#if entry.approvedByName}
											· approved by {entry.approvedByName}
										{/if}
										· {format.dateTime(entry.at)}
									</p>
								</div>

								{#if entry.amountMinor !== null}
									<Numeric value={format.moneyPlain(entry.amountMinor)} />
								{/if}

								{#if entry.unapproved}
									<!-- The signal worth waking someone for: they approved themselves, and their
									     role did not carry it. -->
									<Badge tone="danger" dot>Self-approved</Badge>
								{:else}
									<Badge tone={severityTone(entry.severity)}>{entry.severity}</Badge>
								{/if}
							</div>
						{/each}
					{/if}
				</Card>
			</div>

			<div class="flex flex-col gap-4">
				<Card label={firstPerson ? 'Enrol the first person' : 'Enrol someone'}>
					<div class="flex flex-col gap-3">
						{#if firstPerson}
							<p class="text-secondary text-text-secondary">
								The first person needs nobody's approval, because there is nobody to give it. They
								must be an owner: only an owner can enrol anyone, so a first cashier would leave
								this outlet permanently unable to add staff.
							</p>
						{/if}

						<Field id="staff-name" label="Name">
							{#snippet children({ id, describedBy })}
								<Input {id} {describedBy} bind:value={name} placeholder="Ruma" />
							{/snippet}
						</Field>

						<Field id="staff-role" label="Role">
							{#snippet children({ id, describedBy })}
								<select
									{id}
									aria-describedby={describedBy}
									bind:value={role}
									disabled={firstPerson}
									class="border-border bg-surface text-body w-full border px-3 disabled:opacity-50"
									style="min-height: var(--scale-touch-target)"
								>
									{#each ROLES as entry (entry.value)}
										<option value={entry.value}>{entry.label}</option>
									{/each}
								</select>
							{/snippet}
						</Field>

						<p class="text-secondary text-text-muted">
							{ROLES.find((entry) => entry.value === role)?.note}
						</p>

						<Field id="staff-pin" label="PIN" hint="4 to 8 digits. Not 1234 or 0000.">
							{#snippet children({ id, describedBy })}
								<Input {id} {describedBy} bind:value={newPin} numeric forceLtr placeholder="8317" />
							{/snippet}
						</Field>

						<Button variant="primary" size="lg" onclick={startEnrol} disabled={busy}>Enrol</Button>
					</div>
				</Card>
			</div>
		</div>
	{/if}

	{#if pendingEnrol}
		<PinPrompt
			action="Add {name.trim() || 'a staff member'} to this outlet"
			{busy}
			error={approvalError}
			onsubmit={confirmEnrol}
			oncancel={() => {
				pendingEnrol = false;
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
