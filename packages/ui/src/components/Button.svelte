<script lang="ts">
	import type { Snippet } from 'svelte';

	type Variant = 'primary' | 'secondary' | 'ghost' | 'danger' | 'link';
	type Size = 'xs' | 'sm' | 'md' | 'lg';

	interface Props {
		variant?: Variant;
		size?: Size;
		loading?: boolean;
		disabled?: boolean;
		type?: 'button' | 'submit' | 'reset';
		/** Stretch to the container — tender buttons on the sell screen. */
		block?: boolean;
		class?: string;
		onclick?: (event: MouseEvent) => void;
		children: Snippet;
	}

	let {
		variant = 'secondary',
		size = 'md',
		loading = false,
		disabled = false,
		type = 'button',
		block = false,
		class: extraClass = '',
		onclick,
		children
	}: Props = $props();

	/*
	 * Height comes from `--scale-control-height`, which the density mode drives. That is what gives
	 * the same component a 32px row in the dashboard and a 44px touch target on the sell screen
	 * without a second implementation — and 44px is a floor, not a preference: a mis-tap at a counter
	 * costs money and trust.
	 *
	 * Note it references the raw `--scale-*` property, **not** a Tailwind `@theme` alias. `@theme
	 * inline` bakes a variable's value in at build time, so an alias read from a `style` attribute
	 * stays frozen at the compact size and the touch target silently never grows.
	 */
	const VARIANT_CLASS: Record<Variant, string> = {
		primary:
			'bg-primary text-white border border-transparent hover:bg-primary-hover active:brightness-95',
		secondary:
			'bg-surface text-text border border-border hover:bg-surface-hover active:brightness-95',
		ghost: 'bg-transparent text-text border border-transparent hover:bg-surface-hover',
		danger:
			'bg-danger text-white border border-transparent hover:bg-danger-hover active:brightness-95',
		link: 'bg-transparent text-primary-text border border-transparent underline-offset-2 hover:underline p-0'
	};

	const SIZE_CLASS: Record<Size, string> = {
		xs: 'text-secondary px-2 gap-1',
		sm: 'text-secondary px-2.5 gap-1.5',
		md: 'text-body px-3 gap-2',
		lg: 'text-md px-4 gap-2'
	};

	let isInert = $derived(disabled || loading);
</script>

<button
	{type}
	class="inline-flex cursor-pointer items-center justify-center rounded-[var(--radius-control)]
	       font-medium whitespace-nowrap transition-[background-color,border-color,filter]
	       duration-100 disabled:cursor-not-allowed disabled:opacity-50
	       {VARIANT_CLASS[variant]} {SIZE_CLASS[size]} {block ? 'w-full' : ''} {extraClass}"
	style={variant === 'link' ? undefined : 'min-height: var(--scale-control-height)'}
	disabled={isInert}
	aria-busy={loading}
	{onclick}
>
	{#if loading}
		<!-- aria-hidden: the spinner is decorative, `aria-busy` carries the meaning to a screen reader. -->
		<svg
			class="size-[1em] animate-spin"
			viewBox="0 0 16 16"
			fill="none"
			aria-hidden="true"
			focusable="false"
		>
			<circle cx="8" cy="8" r="6.5" stroke="currentColor" stroke-opacity="0.25" stroke-width="2" />
			<path
				d="M14.5 8A6.5 6.5 0 0 0 8 1.5"
				stroke="currentColor"
				stroke-width="2"
				stroke-linecap="round"
			/>
		</svg>
	{/if}
	{@render children()}
</button>
