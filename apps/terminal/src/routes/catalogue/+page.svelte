<script lang="ts">
	/**
	 * What the shop sells.
	 *
	 * The price here is the *current* price. Every sale line snapshots what it charged at the time,
	 * so editing this never rewrites history — which is exactly what makes last-writer-wins safe
	 * when two devices edit the same product while apart.
	 *
	 * Withdrawing takes a product off the sell screen without deleting it. Past sales reference it,
	 * and a sale pointing at nothing is a report nobody can read and a recall nobody can trace.
	 */
	import {
		Badge,
		Button,
		Card,
		Field,
		Input,
		Select,
		createFormatters,
		minorToDecimalString,
		parseMinor
	} from '@sahl/ui';
	import PinPrompt from '$lib/PinPrompt.svelte';
	import {
		asTillError,
		isTillAvailable,
		till,
		type ModifierGroup,
		type ProductView,
		type TaxTreatment
	} from '$lib/till';

	const format = createFormatters({ locale: 'en', currency: 'BDT', timeZone: 'Asia/Dhaka' });

	const UNITS = [
		{ value: 'pcs', label: 'Pieces' },
		{ value: 'kg', label: 'Kilograms' },
		{ value: 'g', label: 'Grams' },
		{ value: 'L', label: 'Litres' },
		{ value: 'ml', label: 'Millilitres' },
		{ value: 'm', label: 'Metres' },
		{ value: 'pack', label: 'Packs' }
	];

	const TREATMENTS: Array<{ value: TaxTreatment; label: string }> = [
		{ value: 'standard', label: 'Standard rate' },
		{ value: 'zero_rated', label: 'Zero-rated' },
		{ value: 'exempt', label: 'Exempt' }
	];

	/** Bangladesh's rate ladder. */
	const RATES = [
		{ value: '1500', label: '15%' },
		{ value: '1000', label: '10%' },
		{ value: '750', label: '7.5%' },
		{ value: '500', label: '5%' },
		{ value: '450', label: '4.5%' },
		{ value: '240', label: '2.4%' }
	];

	let products = $state<ProductView[]>([]);
	let error = $state<{ code: string; message: string } | null>(null);
	let busy = $state(false);
	let available = $state(true);

	/** The product being edited, or `null` when adding a new one. */
	let editing = $state<ProductView | null>(null);
	let showForm = $state(false);

	let name = $state('');
	let sku = $state('');
	let barcodes = $state('');
	let price = $state('');
	let unit = $state('pcs');
	let treatment = $state<TaxTreatment>('standard');
	let rate = $state('1500');
	let category = $state('');
	/** Option groups being edited. Ids are null for anything newly added; the till mints them. */
	let groups = $state<
		Array<{
			id: string | null;
			name: string;
			min: number;
			max: number;
			options: Array<{ id: string | null; name: string; price: string }>;
		}>
	>([]);

	let pendingSave = $state(false);
	let pendingToggle = $state<ProductView | null>(null);
	let approvalError = $state<string | null>(null);

	let search = $state('');
	let visible = $derived(
		search.trim()
			? products.filter((product) => {
					const needle = search.trim().toLowerCase();
					return (
						product.name.toLowerCase().includes(needle) ||
						product.sku?.toLowerCase().includes(needle) ||
						product.barcodes.some((code) => code.includes(needle))
					);
				})
			: products
	);
	let withdrawn = $derived(products.filter((product) => !product.active).length);

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
				() => till.allProducts(),
				(result) => (products = result)
			);
		}
	});

	function startAdd() {
		editing = null;
		name = '';
		sku = '';
		barcodes = '';
		price = '';
		unit = 'pcs';
		treatment = 'standard';
		rate = '1500';
		category = '';
		groups = [];
		showForm = true;
	}

	function startEdit(product: ProductView) {
		editing = product;
		name = product.name;
		sku = product.sku ?? '';
		barcodes = product.barcodes.join(', ');
		// Digit manipulation, never division. `priceMinor / 100` is the float this codebase exists
		// to avoid, and it would round a price before anyone saw it.
		price = minorToDecimalString(product.priceMinor, 2);
		unit = product.unit;
		treatment = product.taxTreatment;
		rate = String(product.taxBasisPoints || 1500);
		category = product.category ?? '';
		groups = product.optionGroups.map((group) => ({
			id: group.id,
			name: group.name,
			min: group.min,
			max: group.max,
			options: group.options.map((option) => ({
				id: option.id,
				name: option.name,
				price: minorToDecimalString(option.priceDeltaMinor, 2)
			}))
		}));
		showForm = true;
	}

	function addGroup() {
		// Defaults to a required single choice, because "size" is the group almost everyone adds
		// first and the one that is wrong as a multi-select.
		groups = [
			...groups,
			{ id: null, name: '', min: 1, max: 1, options: [{ id: null, name: '', price: '0' }] }
		];
	}

	function startSave() {
		if (!name.trim()) {
			error = { code: 'bad_name', message: 'Enter a product name' };
			return;
		}
		const priceMinor = parseMinor(price, 'BDT');
		if (priceMinor === null || priceMinor < 0) {
			error = { code: 'bad_price', message: 'Enter a price like 480 or 479.50' };
			return;
		}
		approvalError = null;
		pendingSave = true;
	}

	function confirmSave(pin: string) {
		const priceMinor = parseMinor(price, 'BDT');
		if (priceMinor === null) return;

		void (async () => {
			busy = true;
			approvalError = null;
			try {
				products = await till.saveProduct({
					productId: editing?.id ?? null,
					name,
					sku: sku.trim() || null,
					barcodes: barcodes
						.split(',')
						.map((code) => code.trim())
						.filter(Boolean),
					priceMinor,
					optionGroups: groups.map((group) => ({
						id: group.id,
						name: group.name,
						min: group.min,
						max: group.max,
						options: group.options.map((option) => ({
							id: option.id,
							name: option.name,
							// A delta can be negative — "no cheese, less 20" — so this parses signed.
							priceDeltaMinor: parseMinor(option.price, 'BDT') ?? 0
						}))
					})),
					unit,
					// Only read for a standard supply; zero-rated and exempt carry no rate.
					taxBasisPoints: treatment === 'standard' ? Number(rate) : 0,
					taxTreatment: treatment,
					category: category.trim() || null,
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
				products = await till.setProductActive(target.id, !target.active, pin);
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
</script>

<svelte:head><title>Catalogue — Sahl</title></svelte:head>

<main class="bg-canvas text-text flex h-dvh flex-col" data-density="compact">
	<header class="border-border bg-surface flex items-center justify-between border-b px-4 py-3">
		<div class="flex items-center gap-3">
			<h1 class="text-lg font-semibold">Catalogue</h1>
			<Badge tone="neutral">{format.integer(products.length)} products</Badge>
			{#if withdrawn > 0}
				<Badge tone="neutral">{format.integer(withdrawn)} withdrawn</Badge>
			{/if}
		</div>
		<div class="flex gap-4">
			<a href="/stock" class="text-secondary text-text-secondary hover:text-text underline">Stock</a
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
		<div class="grid min-h-0 flex-1 grid-cols-1 gap-4 overflow-y-auto p-4 lg:grid-cols-[1fr_24rem]">
			<div class="flex min-h-0 flex-col gap-3">
				<div class="flex items-end gap-2">
					<Field id="catalogue-search" label="Find" class="flex-1">
						{#snippet children({ id, describedBy })}
							<Input {id} {describedBy} bind:value={search} placeholder="Name, SKU or barcode" />
						{/snippet}
					</Field>
					<Button variant="primary" onclick={startAdd} disabled={busy}>Add a product</Button>
				</div>

				<Card label="Products" flush>
					{#if products.length === 0}
						<p class="text-secondary text-text-muted p-4">
							Nothing yet. Until a product exists the sell screen has nothing to tap, and a challan
							cannot say what unit a supply was counted in.
						</p>
					{:else if visible.length === 0}
						<p class="text-secondary text-text-muted p-4">Nothing matches “{search}”.</p>
					{:else}
						{#each visible as product (product.id)}
							<div
								class="border-border flex items-center gap-3 border-b px-3"
								style="min-height: var(--scale-row-height)"
							>
								<div class="min-w-0 flex-1">
									<p class="text-body truncate {product.active ? '' : 'text-text-muted'}">
										{product.name}
									</p>
									<p class="text-secondary text-text-muted truncate">
										{product.sku ?? 'No SKU'}
										{#if product.barcodes.length > 0}
											· {product.barcodes.join(', ')}
										{/if}
										· per {product.unit}
									</p>
								</div>

								{#if product.taxTreatment === 'exempt'}
									<Badge tone="neutral">Exempt</Badge>
								{:else if product.taxTreatment === 'zero_rated'}
									<Badge tone="neutral">Zero-rated</Badge>
								{:else}
									<Badge tone="neutral">{format.percent(product.taxBasisPoints)}</Badge>
								{/if}

								{#if !product.active}
									<Badge tone="neutral" dot>Withdrawn</Badge>
								{/if}

								<span class="numeric text-body">{format.moneyPlain(product.priceMinor)}</span>

								<div class="flex gap-1">
									<Button
										variant="ghost"
										size="xs"
										onclick={() => startEdit(product)}
										disabled={busy}
									>
										Edit
									</Button>
									<Button
										variant="ghost"
										size="xs"
										onclick={() => {
											approvalError = null;
											pendingToggle = product;
										}}
										disabled={busy}
									>
										{product.active ? 'Withdraw' : 'Restore'}
									</Button>
								</div>
							</div>
						{/each}
					{/if}
				</Card>
			</div>

			<div class="flex flex-col gap-4">
				{#if showForm}
					<Card label={editing ? 'Edit product' : 'New product'}>
						<div class="flex flex-col gap-3">
							<Field id="product-name" label="Name">
								{#snippet children({ id, describedBy })}
									<Input {id} {describedBy} bind:value={name} placeholder="Basmati rice 5kg" />
								{/snippet}
							</Field>

							<div class="grid grid-cols-2 gap-3">
								<Field id="product-price" label="Price">
									{#snippet children({ id, describedBy })}
										<Input
											{id}
											{describedBy}
											bind:value={price}
											numeric
											forceLtr
											placeholder="480"
										/>
									{/snippet}
								</Field>

								<Field id="product-unit" label="Sold by" hint="Prints on the challan.">
									{#snippet children({ id, describedBy })}
										<Select {id} {describedBy} bind:value={unit} options={UNITS} />
									{/snippet}
								</Field>
							</div>

							<Field id="product-treatment" label="VAT treatment">
								{#snippet children({ id, describedBy })}
									<Select {id} {describedBy} bind:value={treatment} options={TREATMENTS} />
								{/snippet}
							</Field>

							{#if treatment === 'standard'}
								<Field id="product-rate" label="Rate">
									{#snippet children({ id, describedBy })}
										<Select {id} {describedBy} bind:value={rate} options={RATES} />
									{/snippet}
								</Field>
							{:else}
								<p class="text-secondary text-text-muted">
									{treatment === 'exempt'
										? 'Outside VAT — input VAT on it cannot be reclaimed.'
										: 'Taxable at 0% — input VAT remains reclaimable.'}
								</p>
							{/if}

							<Field id="product-sku" label="SKU" hint="Optional. The shop's own code.">
								{#snippet children({ id, describedBy })}
									<Input
										{id}
										{describedBy}
										bind:value={sku}
										numeric
										forceLtr
										placeholder="RICE-5"
									/>
								{/snippet}
							</Field>

							<Field
								id="product-barcodes"
								label="Barcodes"
								hint="Comma-separated. The same good from two importers carries two."
							>
								{#snippet children({ id, describedBy })}
									<Input
										{id}
										{describedBy}
										bind:value={barcodes}
										numeric
										forceLtr
										placeholder="8901234567890"
									/>
								{/snippet}
							</Field>

							<Field id="product-category" label="Category" hint="Optional.">
								{#snippet children({ id, describedBy })}
									<Input {id} {describedBy} bind:value={category} placeholder="Staples" />
								{/snippet}
							</Field>

							<div class="border-border flex flex-col gap-3 border-t pt-3">
								<div class="flex items-baseline justify-between">
									<span class="label-caps">Options</span>
									<Button variant="ghost" size="xs" onclick={addGroup} disabled={busy}>
										Add a group
									</Button>
								</div>

								{#if groups.length === 0}
									<p class="text-secondary text-text-muted">
										None. A product with no options is rung with one tap; one with options asks
										first.
									</p>
								{/if}

								{#each groups as group, groupIndex (groupIndex)}
									<div class="border-border flex flex-col gap-2 border p-3">
										<div class="flex items-end gap-2">
											<Field id="group-name-{groupIndex}" label="Group" class="flex-1">
												{#snippet children({ id, describedBy })}
													<Input {id} {describedBy} bind:value={group.name} placeholder="Size" />
												{/snippet}
											</Field>
											<Button
												variant="ghost"
												size="xs"
												onclick={() => groups.splice(groupIndex, 1)}
												disabled={busy}
											>
												Remove
											</Button>
										</div>

										<Field
											id="group-kind-{groupIndex}"
											label="How many"
											hint="A size is one of; extras are any number."
										>
											{#snippet children({ id, describedBy })}
												<Select
													{id}
													{describedBy}
													value={group.min === 1 && group.max === 1
														? 'one'
														: group.max === 1
															? 'upto-one'
															: 'many'}
													onchange={(event) => {
														const kind = (event.target as HTMLSelectElement).value;
														// Bounds rather than a mode flag, because the till validates
														// against min and max and a second representation could disagree.
														if (kind === 'one') {
															group.min = 1;
															group.max = 1;
														} else if (kind === 'upto-one') {
															group.min = 0;
															group.max = 1;
														} else {
															group.min = 0;
															group.max = Math.max(2, group.options.length);
														}
													}}
													options={[
														{ value: 'one', label: 'Exactly one — required' },
														{ value: 'upto-one', label: 'Up to one — optional' },
														{ value: 'many', label: 'Any number' }
													]}
												/>
											{/snippet}
										</Field>

										{#each group.options as option, optionIndex (optionIndex)}
											<div class="flex items-end gap-2">
												<Field
													id="option-name-{groupIndex}-{optionIndex}"
													label="Choice"
													class="flex-1"
												>
													{#snippet children({ id, describedBy })}
														<Input
															{id}
															{describedBy}
															bind:value={option.name}
															placeholder="Large"
														/>
													{/snippet}
												</Field>
												<Field
													id="option-price-{groupIndex}-{optionIndex}"
													label="Adds"
													class="w-24"
												>
													{#snippet children({ id, describedBy })}
														<Input
															{id}
															{describedBy}
															bind:value={option.price}
															numeric
															forceLtr
															placeholder="0"
														/>
													{/snippet}
												</Field>
												{#if group.options.length > 1}
													<Button
														variant="ghost"
														size="xs"
														onclick={() => group.options.splice(optionIndex, 1)}
														disabled={busy}
													>
														×
													</Button>
												{/if}
											</div>
										{/each}

										<Button
											variant="ghost"
											size="xs"
											onclick={() => {
												group.options.push({ id: null, name: '', price: '0' });
												// Keep an "any number" group able to accept everything in it.
												if (group.max > 1) group.max = group.options.length;
											}}
											disabled={busy}
										>
											Add a choice
										</Button>
									</div>
								{/each}
							</div>

							<div class="flex gap-2">
								<Button variant="primary" onclick={startSave} disabled={busy}>
									{editing ? 'Save changes' : 'Add product'}
								</Button>
								<Button variant="ghost" onclick={() => (showForm = false)} disabled={busy}>
									Cancel
								</Button>
							</div>
						</div>
					</Card>
				{:else}
					<Card label="About prices">
						<p class="text-secondary text-text-secondary">
							The price here is the current price. Every sale records what it actually charged, so
							changing one never rewrites a sale that already happened — which is what makes it safe
							for two tills to edit the same product while they are apart.
						</p>
					</Card>
				{/if}
			</div>
		</div>
	{/if}

	{#if pendingSave}
		<PinPrompt
			action={editing ? `Change ${editing.name}` : 'Add a product to the catalogue'}
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
				? `Take ${pendingToggle.name} off the sell screen`
				: `Put ${pendingToggle.name} back on the sell screen`}
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
