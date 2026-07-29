<script lang="ts">
	import type { Snippet } from 'svelte';

	/**
	 * A small status chip.
	 *
	 * The register-state tones — `offline`, `unsynced`, `voided`, `low-stock` — are the reason this
	 * exists as a component rather than a utility class. Those states are *data*: a cashier reads
	 * them in a glance from across a counter, and an owner spots a voided line in a list of two
	 * hundred. Centralising them keeps that meaning identical everywhere it appears.
	 */
	type Tone =
		| 'neutral'
		| 'primary'
		| 'success'
		| 'warn'
		| 'danger'
		| 'offline'
		| 'unsynced'
		| 'voided'
		| 'low-stock';

	interface Props {
		tone?: Tone;
		/** Show a leading dot — useful where the label alone is ambiguous at a distance. */
		dot?: boolean;
		class?: string;
		children: Snippet;
	}

	let { tone = 'neutral', dot = false, class: extraClass = '', children }: Props = $props();

	const TONE_CLASS: Record<Tone, string> = {
		neutral: 'bg-surface-sunken text-text-secondary border-border',
		primary: 'bg-primary-subtle text-primary-text border-transparent',
		success: 'bg-success-subtle text-success-text border-transparent',
		warn: 'bg-warn-subtle text-warn-text border-transparent',
		danger: 'bg-danger-subtle text-danger-text border-transparent',
		offline: 'bg-offline-subtle text-warn-text border-transparent',
		unsynced: 'bg-unsynced-subtle text-primary-text border-transparent',
		voided: 'bg-voided-subtle text-danger-text border-transparent',
		'low-stock': 'bg-warn-subtle text-warn-text border-transparent'
	};

	const DOT_CLASS: Record<Tone, string> = {
		neutral: 'bg-text-muted',
		primary: 'bg-primary',
		success: 'bg-success',
		warn: 'bg-warn',
		danger: 'bg-danger',
		offline: 'bg-offline',
		unsynced: 'bg-unsynced',
		voided: 'bg-voided',
		'low-stock': 'bg-low-stock'
	};
</script>

<span
	class="text-label inline-flex items-center gap-1.5 rounded-none border px-2 py-0.5 font-semibold
	       whitespace-nowrap {TONE_CLASS[tone]} {extraClass}"
>
	{#if dot}
		<span class="size-1.5 {DOT_CLASS[tone]}" aria-hidden="true"></span>
	{/if}
	{@render children()}
</span>
