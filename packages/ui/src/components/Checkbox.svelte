<script lang="ts">
	/**
	 * A checkbox that matches [`Input`] and [`Select`].
	 *
	 * The native control cannot be styled to this design system — square corners, hairline borders,
	 * the indigo accent — on every platform, and `accent-color` alone leaves the box itself looking
	 * like whatever the OS thinks. So the real input stays, visually hidden but focusable and
	 * announced, and the square beside it is drawn.
	 *
	 * Drawn rather than a background image because a rendered SVG inherits `currentColor` and a
	 * data URI cannot. That is the whole reason this can carry the theme without a hardcoded hex.
	 */
	import Check from '@lucide/svelte/icons/check';

	interface Props {
		id?: string;
		checked?: boolean;
		disabled?: boolean;
		describedBy?: string;
		/** The label beside the box. Clicking it toggles, because a 16px target does not. */
		label?: string;
		class?: string;
	}

	let {
		id,
		checked = $bindable(false),
		disabled = false,
		describedBy,
		label,
		class: extraClass = ''
	}: Props = $props();
</script>

<label
	class="flex items-start gap-2 {disabled
		? 'cursor-not-allowed opacity-60'
		: 'cursor-pointer'} {extraClass}"
>
	<!-- Not `hidden`: a hidden input is not focusable and not announced. Sized to the box it sits
	     under so a click anywhere on the square reaches it. -->
	<input
		{id}
		type="checkbox"
		bind:checked
		{disabled}
		aria-describedby={describedBy}
		class="peer sr-only"
	/>

	<span
		class="border-border bg-surface text-inverse peer-focus-visible:ring-focus mt-0.5 flex
		       size-4 shrink-0 items-center justify-center border transition-colors duration-100
		       peer-checked:border-[var(--color-primary)] peer-checked:bg-[var(--color-primary)]
		       peer-focus-visible:ring-2 peer-focus-visible:ring-offset-1"
		aria-hidden="true"
	>
		{#if checked}
			<Check size={12} strokeWidth={3} />
		{/if}
	</span>

	{#if label}
		<span class="text-body select-none">{label}</span>
	{/if}
</label>
