<script lang="ts">
	import type { Snippet } from 'svelte';

	/**
	 * A white panel on the canvas.
	 *
	 * Structure is carried by a hairline border rather than a shadow, which is what keeps the dense
	 * reference look from turning soft — and what keeps many panels on one screen legible instead of
	 * muddy.
	 */
	interface Props {
		/** Optional uppercase micro-label above the panel. */
		label?: string;
		/** Remove interior padding — for tables that should meet the border. */
		flush?: boolean;
		class?: string;
		/**
		 * Classes for the bordered body, not the outer section.
		 *
		 * Needed because the body is a separate element: making the section a flex column does not
		 * reach the content, so a scroll area inside a `flex-1` Card never gets a bounded height and
		 * grows past the viewport instead of scrolling.
		 */
		bodyClass?: string;
		children: Snippet;
		actions?: Snippet;
	}

	let {
		label,
		flush = false,
		class: extraClass = '',
		bodyClass = '',
		children,
		actions
	}: Props = $props();
</script>

<section class={extraClass}>
	{#if label || actions}
		<header class="mb-2 flex items-center justify-between gap-2">
			{#if label}<h2 class="label-caps">{label}</h2>{/if}
			{#if actions}<div class="flex items-center gap-2">{@render actions()}</div>{/if}
		</header>
	{/if}
	<div
		class="border-border bg-surface rounded-[var(--radius-panel)] border {flush
			? ''
			: 'p-4'} {bodyClass}"
	>
		{@render children()}
	</div>
</section>
