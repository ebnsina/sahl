<script lang="ts" generics="T extends string">
	/**
	 * A dropdown that matches [`Input`] exactly.
	 *
	 * Built because ten screens had hand-rolled one and no two agreed. Height came from three
	 * different tokens, padding was `px-3` beside an input's `px-2.5`, and none of them had the
	 * hover border, disabled treatment or invalid state that `Input` has. A select and a text field
	 * on the same form looked like they came from different products.
	 *
	 * Height is `--scale-control-height`, the same as `Input` and `Button` — **not**
	 * `--scale-touch-target`, which six of the copies used and which is *smaller* than the control
	 * height in compact density. Those selects rendered 28px tall beside 32px inputs.
	 *
	 * The arrow is drawn rather than the platform's. A *rendered* SVG inherits `currentColor`, so
	 * it carries the theme with no hardcoded hex — only a data-URI background could not, which is
	 * what made the native one look imported from another product beside every other control here.
	 */
	import ChevronDown from '@lucide/svelte/icons/chevron-down';

	interface Props<Value> {
		id?: string;
		value?: Value;
		options: ReadonlyArray<{ value: Value; label: string }>;
		disabled?: boolean;
		invalid?: boolean;
		describedBy?: string;
		class?: string;
		onchange?: (event: Event) => void;
	}

	let {
		id,
		value = $bindable(),
		options,
		disabled = false,
		invalid = false,
		describedBy,
		class: extraClass = '',
		onchange
	}: Props<T> = $props();
</script>

<div class="relative w-full">
	<select
		{id}
		{disabled}
		bind:value
		aria-invalid={invalid || undefined}
		aria-describedby={describedBy}
		class="bg-surface text-body text-text disabled:bg-surface-sunken w-full appearance-none
		       rounded-[var(--radius-control)] border py-0 ps-2.5 pe-8
		       transition-[border-color,box-shadow] duration-100 disabled:cursor-not-allowed
		       disabled:opacity-60
		       {invalid ? 'border-danger' : 'border-border hover:border-border-strong'}
		       {extraClass}"
		style="min-height: var(--scale-control-height)"
		{onchange}
	>
		{#each options as option (option.value)}
			<option value={option.value}>{option.label}</option>
		{/each}
	</select>

	<!-- `end` rather than `right`: the terminal runs in Arabic, where the arrow belongs on the
	     left and the text starts on the right. -->
	<span
		class="text-text-muted pointer-events-none absolute inset-y-0 end-2 flex items-center"
		aria-hidden="true"
	>
		<ChevronDown size={16} />
	</span>
</div>
