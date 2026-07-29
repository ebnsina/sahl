<script lang="ts">
	/**
	 * The Sahl mark.
	 *
	 * *Sahl* (سهل) means "easy". The mark is a receipt reduced to three strokes — the one object
	 * every merchant in every vertical recognises, and the one thing this product is ultimately for.
	 *
	 * Square corners and a single flat accent, matching the rest of the system: no radii, no
	 * gradients, no shadow. It has to stay legible at 16px in a browser tab and at 512px in an
	 * installer, so there is nothing in it that thin strokes would lose.
	 *
	 * Drawn inline rather than loaded as a file: the terminal is offline-first, and an `<img>` that
	 * fails is a blank square in the corner of a register.
	 */
	interface Props {
		/** Rendered size in px. The mark is square. */
		size?: number;
		/** Show the wordmark beside the glyph. */
		withWordmark?: boolean;
		class?: string;
	}

	let { size = 24, withWordmark = false, class: extraClass = '' }: Props = $props();
</script>

<span class="inline-flex items-center gap-2 {extraClass}">
	<svg
		width={size}
		height={size}
		viewBox="0 0 32 32"
		fill="none"
		role="img"
		aria-label="Sahl"
		style="flex-shrink: 0"
	>
		<rect width="32" height="32" fill="var(--color-primary)" />
		<!-- Three strokes: two full lines and a short one, the shape of a receipt with a total. -->
		<rect x="7" y="9" width="18" height="3" fill="white" />
		<rect x="7" y="15" width="18" height="3" fill="white" />
		<rect x="7" y="21" width="9" height="3" fill="white" />
	</svg>

	{#if withWordmark}
		<span class="text-md font-semibold tracking-tight">Sahl</span>
	{/if}
</span>
