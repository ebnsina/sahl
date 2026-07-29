<script lang="ts">
	interface Props {
		id?: string;
		value?: string;
		type?: 'text' | 'search' | 'password' | 'email' | 'tel';
		placeholder?: string;
		disabled?: boolean;
		invalid?: boolean;
		describedBy?: string;
		/**
		 * Render in the mono tabular face — for barcodes, SKUs, reference numbers and any field that
		 * holds digits the user will scan down a column.
		 */
		numeric?: boolean;
		/** Force LTR regardless of page direction. Barcodes and SKUs are never RTL, even in Arabic. */
		forceLtr?: boolean;
		class?: string;
		oninput?: (event: Event) => void;
	}

	let {
		id,
		value = $bindable(''),
		type = 'text',
		placeholder,
		disabled = false,
		invalid = false,
		describedBy,
		numeric = false,
		forceLtr = false,
		class: extraClass = '',
		oninput
	}: Props = $props();
</script>

<input
	{id}
	{type}
	{placeholder}
	{disabled}
	bind:value
	aria-invalid={invalid || undefined}
	aria-describedby={describedBy}
	dir={forceLtr ? 'ltr' : undefined}
	class="bg-surface text-body text-text placeholder:text-text-muted disabled:bg-surface-sunken w-full rounded-[var(--radius-control)]
	       border px-2.5
	       transition-[border-color,box-shadow] duration-100 disabled:cursor-not-allowed
	       disabled:opacity-60
	       {invalid ? 'border-danger' : 'border-border hover:border-border-strong'}
	       {numeric ? 'numeric' : ''} {extraClass}"
	style="min-height: var(--scale-control-height)"
	{oninput}
/>
