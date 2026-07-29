<script lang="ts">
	import type { Snippet } from 'svelte';

	/**
	 * A labelled form control with hint and error slots.
	 *
	 * Errors are part of the component rather than something each screen improvises, because the
	 * conventions have to hold everywhere: the message sits below the control, `aria-describedby`
	 * and `aria-invalid` are wired without the caller thinking about it, and an errored field never
	 * relies on colour alone to say so.
	 */
	interface Props {
		label: string;
		/** DOM id shared by the control and its description — required to wire ARIA. */
		id: string;
		hint?: string;
		error?: string;
		required?: boolean;
		class?: string;
		children: Snippet<[{ id: string; describedBy: string | undefined; invalid: boolean }]>;
	}

	let {
		label,
		id,
		hint,
		error,
		required = false,
		class: extraClass = '',
		children
	}: Props = $props();

	let describedBy = $derived(error ? `${id}-error` : hint ? `${id}-hint` : undefined);
</script>

<div class="flex flex-col gap-1 {extraClass}">
	<label for={id} class="text-secondary text-text font-medium">
		{label}
		{#if required}<span class="text-danger-text" aria-hidden="true">*</span>{/if}
	</label>

	{@render children({ id, describedBy, invalid: Boolean(error) })}

	{#if error}
		<!-- role="alert" so the message is announced when it appears mid-form. -->
		<p id="{id}-error" class="text-secondary text-danger-text" role="alert">{error}</p>
	{:else if hint}
		<p id="{id}-hint" class="text-secondary text-text-muted">{hint}</p>
	{/if}
</div>
