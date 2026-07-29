<script lang="ts">
	/**
	 * Renders any number in Geist Mono with tabular figures.
	 *
	 * Every numeric value in the product goes through this. Tabular figures matter more than they
	 * look: without them digits have different widths, so a column of prices fails to align and a
	 * running total visibly jitters as it updates mid-transaction. `slashed-zero` keeps 0 and O
	 * distinct in SKUs and receipt numbers.
	 *
	 * The value must already be formatted — by `createFormatters`, never by hand. This component
	 * decides how a number *looks*, never what it says.
	 */
	interface Props {
		/** Pre-formatted text from `createFormatters`. */
		value: string;
		/** Right-align, the correct default for money in a table column. */
		align?: 'start' | 'end';
		/** Mute values that carry no weight, like a zero balance. */
		muted?: boolean;
		/** Colour by sign — a negative balance or a refund should read as such instantly. */
		signed?: boolean;
		/** Raw sign source, since the formatted string may be localised or parenthesised. */
		sign?: number;
		class?: string;
	}

	let {
		value,
		align = 'end',
		muted = false,
		signed = false,
		sign = 0,
		class: extraClass = ''
	}: Props = $props();

	let toneClass = $derived(
		muted
			? 'text-text-muted'
			: signed && sign < 0
				? 'text-danger-text'
				: signed && sign > 0
					? 'text-success-text'
					: 'text-text'
	);
</script>

<span
	class="numeric tabular-nums {align === 'end'
		? 'text-end'
		: 'text-start'} {toneClass} {extraClass}">{value}</span
>
