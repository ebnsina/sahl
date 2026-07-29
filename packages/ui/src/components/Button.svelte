<script lang="ts">
	import type { IconProps } from '@lucide/svelte';
	import type { Component, Snippet } from 'svelte';

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
		/**
		 * A Lucide icon component, rendered before the label.
		 *
		 * **Use sparingly.** An icon earns its place on an action a cashier repeats at speed, where
		 * shape recognition beats reading — tender, complete, void. It earns nothing on a
		 * back-office form read carefully once, where it is noise beside an already-clear label, and
		 * it costs something when the closest available glyph means the wrong thing: a warning
		 * triangle on "issue stock" is read before the label is.
		 *
		 * Passed as a component rather than a name string so only the icons a screen actually uses
		 * are bundled — the terminal ships offline and has no business carrying 1,500 unused glyphs.
		 */
		icon?: Component<IconProps>;
		/** Icon-only button. The label still renders, for screen readers. */
		iconOnly?: boolean;
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
		icon: Icon,
		iconOnly = false,
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

	/** Square, so an icon-only control still meets the touch target on both axes. */
	const ICON_ONLY_CLASS: Record<Size, string> = {
		xs: 'text-secondary px-1.5',
		sm: 'text-secondary px-2',
		md: 'text-body px-2.5',
		lg: 'text-md px-3'
	};

	let isInert = $derived(disabled || loading);
</script>

<button
	{type}
	class="inline-flex cursor-pointer items-center justify-center rounded-[var(--radius-control)]
	       font-medium whitespace-nowrap transition-[background-color,border-color,filter]
	       duration-100 disabled:cursor-not-allowed disabled:opacity-50
	       {VARIANT_CLASS[variant]} {iconOnly ? ICON_ONLY_CLASS[size] : SIZE_CLASS[size]}
	       {block ? 'w-full' : ''} {extraClass}"
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
	{:else if Icon}
		<!-- `size` in em rather than px so the icon tracks the density scale with the label, instead
		     of staying 16px while a touch-mode button grows around it. -->
		<Icon size="1.15em" aria-hidden="true" focusable="false" />
	{/if}

	{#if iconOnly}
		<span class="sr-only">{@render children()}</span>
	{:else}
		{@render children()}
	{/if}
</button>
