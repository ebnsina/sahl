<script lang="ts">
	/**
	 * The design system showcase.
	 *
	 * Built in P0 rather than "when there's time", for two reasons. Over twelve months of solo work
	 * it is the cheapest way to stop the UI drifting — every new screen is compared against this page
	 * rather than against memory. And it doubles as the visual regression surface: the three toggles
	 * at the top exercise the three axes most likely to break silently.
	 *
	 * Those toggles are not a demo convenience. **Density** proves one token set really does serve
	 * both a 13px dashboard row and a 44px cashier target. **Theme** proves no component hardcoded a
	 * colour. **Direction** proves the Arabic build works — RTL is designed in from the start here,
	 * because retrofitting it means auditing every margin in the product.
	 */
	import { Badge, Button, Card, Field, Input, Logo, Numeric, createFormatters } from '@sahl/ui';
	import Check from '@lucide/svelte/icons/check';
	import Plus from '@lucide/svelte/icons/plus';
	import Trash2 from '@lucide/svelte/icons/trash-2';
	import PinPrompt from '$lib/PinPrompt.svelte';

	let showPin = $state(false);
	let showPinError = $state(false);

	type Density = 'compact' | 'touch';
	type Theme = 'light' | 'dark';
	type Direction = 'ltr' | 'rtl';

	let density = $state<Density>('compact');
	let theme = $state<Theme>('light');
	let direction = $state<Direction>('ltr');
	let loadingDemo = $state(false);

	let locale = $derived(direction === 'rtl' ? ('ar-SA' as const) : ('en' as const));
	let format = $derived(
		createFormatters({
			locale,
			currency: direction === 'rtl' ? 'SAR' : 'BDT',
			timeZone: direction === 'rtl' ? 'Asia/Riyadh' : 'Asia/Dhaka'
		})
	);

	const TYPE_RAMP = [
		{ token: 'text-3xl', label: '30px / 3xl' },
		{ token: 'text-2xl', label: '24px / 2xl' },
		{ token: 'text-xl', label: '20px / xl' },
		{ token: 'text-lg', label: '17px / lg' },
		{ token: 'text-md', label: '15px / md' },
		{ token: 'text-body', label: '13px / body — the default across the product' }
	];

	const SURFACE_TOKENS = ['canvas', 'surface', 'surface-sunken', 'surface-hover'];
	const ACCENT_TOKENS = ['primary', 'danger', 'warn', 'success'];
	const REGISTER_TOKENS = ['offline', 'unsynced', 'voided', 'low-stock'];

	// A basket showing the shelf-label guarantee: tax-inclusive totals that ring up exactly.
	const BASKET = [
		{ name: 'Rice, 1.234 kg', qtyMilli: 1234, minor: 9872 },
		{ name: 'Cooking oil, 2 L', qtyMilli: 2000, minor: 34000 },
		{ name: 'Bread', qtyMilli: 1000, minor: 5500 }
	];
	const BASKET_TOTAL = BASKET.reduce((sum, line) => sum + line.minor, 0);

	function toggleLoading() {
		loadingDemo = true;
		setTimeout(() => (loadingDemo = false), 1400);
	}
</script>

<svelte:head><title>Design system · Sahl</title></svelte:head>

<div
	data-density={density}
	data-theme={theme}
	dir={direction}
	lang={locale}
	class="bg-canvas text-text min-h-screen"
>
	<header
		class="border-border bg-surface sticky top-0 z-10 flex flex-wrap items-center gap-4 border-b px-6 py-3"
	>
		<h1 class="text-md font-semibold">Design system</h1>

		<div class="ms-auto flex flex-wrap items-center gap-4">
			<fieldset class="flex items-center gap-1.5">
				<legend class="sr-only">Density</legend>
				<span class="label-caps">Density</span>
				{#each ['compact', 'touch'] as const as option (option)}
					<Button
						size="xs"
						variant={density === option ? 'primary' : 'secondary'}
						onclick={() => (density = option)}>{option}</Button
					>
				{/each}
			</fieldset>

			<fieldset class="flex items-center gap-1.5">
				<legend class="sr-only">Theme</legend>
				<span class="label-caps">Theme</span>
				{#each ['light', 'dark'] as const as option (option)}
					<Button
						size="xs"
						variant={theme === option ? 'primary' : 'secondary'}
						onclick={() => (theme = option)}>{option}</Button
					>
				{/each}
			</fieldset>

			<fieldset class="flex items-center gap-1.5">
				<legend class="sr-only">Direction</legend>
				<span class="label-caps">Dir</span>
				{#each ['ltr', 'rtl'] as const as option (option)}
					<Button
						size="xs"
						variant={direction === option ? 'primary' : 'secondary'}
						onclick={() => (direction = option)}>{option}</Button
					>
				{/each}
			</fieldset>
		</div>
	</header>

	<main class="mx-auto flex max-w-5xl flex-col gap-8 px-6 py-8">
		<Card label="Typography">
			<div class="flex flex-col gap-3">
				{#each TYPE_RAMP as entry (entry.token)}
					<p class={entry.token}>
						Hire without the guesswork
						<span class="text-secondary text-text-muted">— {entry.label}</span>
					</p>
				{/each}
				<p class="text-secondary text-text-secondary">Secondary — 12px</p>
				<p class="label-caps">Label — 11px, uppercase, tracked</p>

				<hr class="border-border" />

				<p class="text-secondary text-text-muted">
					Numbers are always Geist Mono with tabular figures, so columns align and a running total
					does not jitter as it updates:
				</p>
				<div class="flex flex-wrap items-baseline gap-x-6 gap-y-2">
					<Numeric value={format.money(16000000)} />
					<Numeric value={format.money(20000000)} />
					<Numeric value={format.date(Date.now())} />
					<Numeric value={format.quantity(1234)} />
					<Numeric value="#A83F91" />
				</div>

				<p class="text-secondary text-text-muted">
					Script coverage is automatic via <code class="numeric">unicode-range</code> — Bangla, Arabic
					and Latin can share a line with no code involved:
				</p>
				<p class="text-md">চাল ৫ কেজি · أرز ٥ كيلو · Rice 5 kg</p>

				<p class="text-secondary text-text-muted">
					Every bundled face carries real weights. This row is here permanently because the failure
					is silent: a single-weight font renders 600 as browser-synthesised bold, which closes the
					counters on Bangla conjuncts at the sizes this UI actually uses.
				</p>
				<div class="flex flex-col gap-1">
					{#each [400, 500, 600, 700] as weight (weight)}
						<p class="text-md" style="font-weight: {weight}">
							চাল ৫ কেজি · أرز ٥ كيلو · Rice 5 kg
							<span class="text-secondary text-text-muted">— {weight}</span>
						</p>
					{/each}
				</div>
			</div>
		</Card>

		<Card label="Buttons">
			<div class="flex flex-col gap-4">
				<div class="flex flex-wrap items-center gap-3">
					<Button variant="primary">primary</Button>
					<Button variant="secondary">secondary</Button>
					<Button variant="ghost">ghost</Button>
					<Button variant="danger">danger</Button>
					<Button variant="link">link</Button>
				</div>
				<div class="flex flex-wrap items-center gap-3">
					<Button size="xs">xs</Button>
					<Button size="sm">sm</Button>
					<Button size="md">md</Button>
					<Button size="lg">lg</Button>
					<Button disabled>disabled</Button>
					<Button variant="primary" loading={loadingDemo} onclick={toggleLoading}>
						Toggle loading
					</Button>
				</div>
			</div>
		</Card>

		<Card label="Form controls">
			<div class="grid gap-4 sm:grid-cols-2">
				<Field id="product" label="Product name" hint="Shown on the receipt">
					{#snippet children({ id, describedBy })}
						<Input {id} {describedBy} placeholder="Basmati rice" />
					{/snippet}
				</Field>

				<Field id="barcode" label="Barcode" hint="Scanner input lands here">
					{#snippet children({ id, describedBy })}
						<Input {id} {describedBy} numeric forceLtr placeholder="8901234567890" />
					{/snippet}
				</Field>

				<Field id="price" label="Unit price" required error="Price cannot be negative">
					{#snippet children({ id, describedBy, invalid })}
						<Input {id} {describedBy} {invalid} numeric forceLtr value="-45.00" />
					{/snippet}
				</Field>

				<Field id="disabled" label="Supplier" hint="Locked while a count is open">
					{#snippet children({ id, describedBy })}
						<Input {id} {describedBy} disabled value="Karim Traders" />
					{/snippet}
				</Field>
			</div>
		</Card>

		<Card label="Register state">
			<div class="flex flex-col gap-4">
				<p class="text-secondary text-text-secondary">
					These tones are data, not decoration. A cashier reads them from across a counter; an owner
					spots a voided line in a list of two hundred.
				</p>
				<div class="flex flex-wrap items-center gap-2">
					<Badge tone="offline" dot>Offline</Badge>
					<Badge tone="unsynced" dot>12 unsynced</Badge>
					<Badge tone="voided">Voided</Badge>
					<Badge tone="low-stock" dot>Low stock</Badge>
					<Badge tone="success" dot>Synced</Badge>
					<Badge tone="neutral">Draft</Badge>
					<Badge tone="primary">15% VAT</Badge>
					<Badge tone="danger">Refund</Badge>
				</div>
			</div>
		</Card>

		<Card label="Mark">
			<div class="flex flex-col gap-4">
				<p class="text-secondary text-text-secondary">
					A receipt reduced to three strokes. Square corners and one flat accent like everything
					else — it has to stay legible at 16px in a tab and at 512px in an installer, so there is
					nothing in it that thin strokes would lose. The app icons are generated from the same four
					rectangles, so the two cannot drift.
				</p>
				<div class="flex items-end gap-6">
					<Logo size={16} />
					<Logo size={24} />
					<Logo size={40} />
					<Logo size={64} />
					<Logo size={32} withWordmark />
				</div>
			</div>
		</Card>

		<Card label="Buttons with icons">
			<div class="flex flex-col gap-3">
				<p class="text-secondary text-text-secondary">
					Icons size in em, so they track the density scale with their label instead of staying 16px
					while a touch-mode button grows around them.
				</p>
				<div class="flex flex-wrap items-center gap-2">
					<Button variant="primary" icon={Plus}>Start a sale</Button>
					<Button variant="secondary" icon={Check}>Confirm</Button>
					<Button variant="danger" icon={Trash2}>Void</Button>
					<Button variant="ghost" icon={Trash2} size="xs">Remove</Button>
					<Button variant="secondary" icon={Check} iconOnly>Confirm</Button>
					<Button variant="primary" icon={Check} loading>Saving</Button>
				</div>
			</div>
		</Card>

		<Card label="Approval prompt">
			<div class="flex flex-col gap-3">
				<p class="text-secondary text-text-secondary">
					Shown whenever an action needs someone other than the cashier. Here because it is the one
					component that never appears in a browser during ordinary development — it only opens
					inside the till — so this is the only place its density, theme and RTL behaviour get
					looked at.
				</p>
				<div class="flex flex-wrap gap-2">
					<Button variant="secondary" onclick={() => (showPin = true)}>Open the prompt</Button>
					<Button variant="secondary" onclick={() => (showPinError = !showPinError)}>
						{showPinError ? 'Clear' : 'Show'} refusal message
					</Button>
				</div>
			</div>
		</Card>

		<Card label="Basket" flush>
			<table class="w-full">
				<caption class="sr-only">Example basket with tax-inclusive pricing</caption>
				<thead>
					<tr class="border-border border-b">
						<th class="label-caps px-4 py-2 text-start">Item</th>
						<th class="label-caps px-4 py-2 text-end">Qty</th>
						<th class="label-caps px-4 py-2 text-end">Amount</th>
					</tr>
				</thead>
				<tbody>
					{#each BASKET as line (line.name)}
						<tr class="border-border border-b last:border-0">
							<td class="text-body px-4" style="height: var(--scale-row-height)">{line.name}</td>
							<td class="px-4 text-end" style="height: var(--scale-row-height)">
								<Numeric value={format.quantity(line.qtyMilli)} />
							</td>
							<td class="px-4 text-end" style="height: var(--scale-row-height)">
								<Numeric value={format.moneyPlain(line.minor)} />
							</td>
						</tr>
					{/each}
				</tbody>
				<tfoot>
					<tr class="bg-surface-sunken">
						<td class="text-body px-4 py-2 font-semibold" colspan="2">Total (VAT included)</td>
						<td class="px-4 py-2 text-end">
							<Numeric value={format.money(BASKET_TOTAL)} class="font-semibold" />
						</td>
					</tr>
				</tfoot>
			</table>
		</Card>

		<Card label="Colour tokens">
			<div class="flex flex-col gap-5">
				{#each [{ title: 'Surfaces', tokens: SURFACE_TOKENS }, { title: 'Accents', tokens: ACCENT_TOKENS }, { title: 'Register', tokens: REGISTER_TOKENS }] as group (group.title)}
					<div class="flex flex-col gap-2">
						<h3 class="label-caps">{group.title}</h3>
						<div class="flex flex-wrap gap-3">
							{#each group.tokens as token (token)}
								<div class="flex flex-col gap-1">
									<div
										class="border-border size-14 rounded-[var(--radius-control)] border bg-{token}"
									></div>
									<span class="text-secondary text-text-muted">{token}</span>
								</div>
							{/each}
						</div>
					</div>
				{/each}
			</div>
		</Card>
	</main>

	{#if showPin}
		<PinPrompt
			action="Void a line from this sale"
			error={showPinError ? 'That PIN was not accepted' : null}
			onsubmit={() => (showPin = false)}
			oncancel={() => (showPin = false)}
		/>
	{/if}
</div>
