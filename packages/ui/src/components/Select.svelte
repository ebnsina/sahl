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
	 * The arrow is the platform's. A custom SVG one would need a hardcoded colour, which this
	 * codebase does not allow in a component and which would be wrong in dark mode besides — a data
	 * URI cannot read a CSS variable. The `color-scheme` token makes the native one follow the
	 * theme, which is the thing that actually needed fixing.
	 */
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

<select
	{id}
	{disabled}
	bind:value
	aria-invalid={invalid || undefined}
	aria-describedby={describedBy}
	class="bg-surface text-body text-text disabled:bg-surface-sunken w-full
	       rounded-[var(--radius-control)] border px-2.5
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
