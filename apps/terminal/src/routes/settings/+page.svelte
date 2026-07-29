<script lang="ts">
	/**
	 * Outlet setup, and what this build is.
	 *
	 * Everything on the left is a thing that must be right on *every* invoice at once rather than
	 * one — the BIN printed on a challan, the timezone a business day closes on. So the till
	 * validates it when it is saved, not when it is used: a Mushak outlet with a blank BIN would
	 * trade all morning and then be unable to issue a single valid challan for the day.
	 *
	 * The right side is the About panel. On a desktop app it is where someone reads the version
	 * back to support, so it carries the device identity and the fiscal position too.
	 */
	import Building2 from '@lucide/svelte/icons/building-2';
	import { Badge, Button, Card, Field, Input, Logo, Select, createFormatters } from '@sahl/ui';
	import PinPrompt from '$lib/PinPrompt.svelte';
	import { asTillError, isTillAvailable, till, type OutletView } from '$lib/till';

	/** Baked in at build time by Vite — the same string the installer carries. */
	const VERSION = __APP_VERSION__;

	const format = createFormatters({ locale: 'en', currency: 'BDT', timeZone: 'Asia/Dhaka' });

	const PROFILES: Array<{ value: OutletView['profile']; label: string; note: string }> = [
		{
			value: 'retail',
			label: 'Retail',
			note: 'Scan, tender, done. Tickets open and close at once.'
		},
		{ value: 'cafe', label: 'Café', note: 'Tables, courses, and tickets that stay open.' },
		{ value: 'grocery', label: 'Grocery', note: 'Weighed goods, batches and expiry dates.' }
	];

	const REGIMES: Array<{ value: OutletView['regime']; label: string; note: string }> = [
		{
			value: 'none',
			label: 'Not VAT-registered',
			note: 'A receipt is the whole obligation. A real setup, not a placeholder.'
		},
		{
			value: 'bd_mushak',
			label: 'Bangladesh — Mushak 6.3',
			note: 'Issues a VAT challan against every sale. Needs a BIN.'
		}
	];

	const TIMEZONES = ['Asia/Dhaka', 'Asia/Riyadh', 'Asia/Dubai', 'UTC'];

	let outlet = $state<OutletView | null>(null);
	let error = $state<{ code: string; message: string } | null>(null);
	let saved = $state(false);
	let busy = $state(false);
	let available = $state(true);

	let name = $state('');
	let profile = $state<OutletView['profile']>('retail');
	let currency = $state('BDT');
	let timezone = $state('Asia/Dhaka');
	let regime = $state<OutletView['regime']>('none');
	let taxRegistration = $state('');
	let address = $state('');

	let pendingSave = $state(false);
	let approvalError = $state<string | null>(null);

	let needsRegistration = $derived(regime === 'bd_mushak');
	let firstSetup = $derived(outlet === null);

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

	function adopt(result: OutletView | null) {
		outlet = result;
		if (!result) return;
		name = result.name;
		profile = result.profile;
		currency = result.currency;
		timezone = result.timezone;
		regime = result.regime;
		taxRegistration = result.taxRegistration ?? '';
		address = result.address;
	}

	$effect(() => {
		available = isTillAvailable();
		if (available) void run(() => till.outletConfig(), adopt);
	});

	function startSave() {
		saved = false;
		if (!name.trim()) {
			error = { code: 'bad_name', message: 'Enter the outlet name' };
			return;
		}
		if (!address.trim()) {
			error = { code: 'bad_address', message: 'Enter the address documents are issued from' };
			return;
		}
		if (needsRegistration && !taxRegistration.trim()) {
			error = { code: 'bad_bin', message: 'Mushak 6.3 needs a BIN before it can issue anything' };
			return;
		}

		// The first setup has nobody to approve it, exactly as the first staff enrolment does.
		if (firstSetup) {
			void run(() => save(''), adoptSaved);
			return;
		}
		approvalError = null;
		pendingSave = true;
	}

	function save(pin: string) {
		return till.configureOutlet({
			name,
			profile,
			currency,
			timezone,
			regime,
			taxRegistration: taxRegistration.trim() || null,
			address,
			pin
		});
	}

	function adoptSaved(result: OutletView | null) {
		adopt(result);
		saved = true;
	}

	function confirmSave(pin: string) {
		void (async () => {
			busy = true;
			approvalError = null;
			try {
				adoptSaved(await save(pin));
				pendingSave = false;
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

	function capabilityLabel(capability: string): string {
		return capability.replaceAll('_', ' ');
	}
</script>

<svelte:head><title>Settings — Sahl</title></svelte:head>

<main class="bg-canvas text-text flex h-dvh flex-col" data-density="compact">
	<header class="border-border bg-surface flex items-center justify-between border-b px-4 py-3">
		<div class="flex items-center gap-3">
			<h1 class="text-lg font-semibold">Settings</h1>
			{#if outlet}
				{@const configured = outlet}
				<Badge tone={configured.regime === 'none' ? 'neutral' : 'primary'}>
					{REGIMES.find((entry) => entry.value === configured.regime)?.label ?? configured.regime}
				</Badge>
			{:else if available}
				<Badge tone="warn" dot>Not set up</Badge>
			{/if}
		</div>
		<div class="flex gap-4">
			<a href="/staff" class="text-secondary text-text-secondary hover:text-text underline">Staff</a
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
		<div class="grid min-h-0 flex-1 grid-cols-1 gap-4 overflow-y-auto p-4 lg:grid-cols-[1fr_22rem]">
			<div class="flex flex-col gap-4">
				<Card label="This outlet">
					<div class="flex flex-col gap-3">
						{#if firstSetup}
							<p class="text-secondary text-text-secondary">
								Nothing is configured yet, so sales are being recorded under no fiscal regime. That
								is a valid way to trade — set a regime here when the shop is registered.
							</p>
						{/if}

						<Field id="outlet-name" label="Outlet name">
							{#snippet children({ id, describedBy })}
								<Input {id} {describedBy} bind:value={name} placeholder="Karim Store — Dhanmondi" />
							{/snippet}
						</Field>

						<Field
							id="outlet-address"
							label="Issuing address"
							hint="Printed on documents. Not always the registered address."
						>
							{#snippet children({ id, describedBy })}
								<Input
									{id}
									{describedBy}
									bind:value={address}
									placeholder="12 Dhanmondi 27, Dhaka 1209"
								/>
							{/snippet}
						</Field>

						<Field id="outlet-profile" label="What kind of shop">
							{#snippet children({ id, describedBy })}
								<Select {id} {describedBy} bind:value={profile} options={PROFILES} />
							{/snippet}
						</Field>
						<p class="text-secondary text-text-muted -mt-2">
							{PROFILES.find((entry) => entry.value === profile)?.note}
						</p>

						<div class="grid grid-cols-2 gap-3">
							<Field id="outlet-currency" label="Currency">
								{#snippet children({ id, describedBy })}
									<Select
										{id}
										{describedBy}
										bind:value={currency}
										options={['BDT', 'SAR', 'AED', 'USD'].map((code) => ({
											value: code,
											label: code
										}))}
									/>
								{/snippet}
							</Field>

							<Field id="outlet-timezone" label="Timezone" hint="Which day a sale belongs to.">
								{#snippet children({ id, describedBy })}
									<Select
										{id}
										{describedBy}
										bind:value={timezone}
										options={TIMEZONES.map((zone) => ({ value: zone, label: zone }))}
									/>
								{/snippet}
							</Field>
						</div>
					</div>
				</Card>

				<Card label="Tax">
					<div class="flex flex-col gap-3">
						<Field id="outlet-regime" label="Fiscal regime">
							{#snippet children({ id, describedBy })}
								<Select {id} {describedBy} bind:value={regime} options={REGIMES} />
							{/snippet}
						</Field>
						<p class="text-secondary text-text-muted -mt-2">
							{REGIMES.find((entry) => entry.value === regime)?.note}
						</p>

						{#if needsRegistration}
							<Field
								id="outlet-bin"
								label="BIN"
								hint="The 13-digit Business Identification Number. Printed on every challan."
							>
								{#snippet children({ id, describedBy })}
									<Input
										{id}
										{describedBy}
										bind:value={taxRegistration}
										numeric
										forceLtr
										placeholder="0031234567890"
									/>
								{/snippet}
							</Field>
						{/if}
					</div>
				</Card>

				<div class="flex items-center gap-3">
					<Button variant="primary" onclick={startSave} disabled={busy}>
						{firstSetup ? 'Complete setup' : 'Save settings'}
					</Button>
					{#if saved}
						<span class="text-secondary text-success-text">Saved.</span>
					{/if}
				</div>
			</div>

			<div class="flex flex-col gap-4">
				<Card label="About">
					<div class="flex flex-col gap-3">
						<Logo size={40} />
						<div>
							<p class="text-md font-semibold">Sahl</p>
							<p class="text-secondary text-text-secondary">
								Offline-first point of sale. The shop keeps selling when the internet does not.
							</p>
						</div>

						<dl class="flex flex-col gap-1">
							{#snippet row(term: string, value: string)}
								<div class="flex items-baseline justify-between gap-3">
									<dt class="text-secondary text-text-muted">{term}</dt>
									<dd class="numeric text-secondary truncate">{value}</dd>
								</div>
							{/snippet}

							{@render row('Version', VERSION)}
							{#if outlet}
								{@render row('Outlet', outlet.outletId.slice(0, 8))}
								{@render row('Set up', format.date(outlet.configuredAt))}
							{/if}
						</dl>

						<p class="text-secondary text-text-muted">© 2026 ebnsina</p>
					</div>
				</Card>

				{#if outlet}
					<Card label="This profile can">
						<div class="flex flex-wrap gap-1.5">
							{#each outlet.capabilities as capability (capability)}
								<Badge tone="neutral">{capabilityLabel(capability)}</Badge>
							{/each}
						</div>
						<p class="text-secondary text-text-muted mt-3">
							Capabilities follow the profile — they are a row, not a setting, so two outlets on the
							same profile always behave the same way.
						</p>
					</Card>
				{/if}

				<Card label="Setup checklist">
					<ul class="flex flex-col gap-2">
						<li class="flex items-start gap-2">
							<Building2 size="1em" class="mt-1 shrink-0" aria-hidden="true" />
							<span class="text-secondary text-text-secondary">
								{outlet ? 'Outlet configured.' : 'Configure the outlet.'}
							</span>
						</li>
					</ul>
				</Card>
			</div>
		</div>
	{/if}

	{#if pendingSave}
		<PinPrompt
			action="Change how this outlet trades"
			{busy}
			error={approvalError}
			onsubmit={confirmSave}
			oncancel={() => {
				pendingSave = false;
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
