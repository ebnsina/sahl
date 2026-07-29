<script lang="ts">
	/**
	 * Ask for a manager's PIN.
	 *
	 * The PIN goes straight to Rust and is cleared the moment the call returns. Nothing here decides
	 * whether it was good enough — the till checks it against every account that actually holds the
	 * permission, so this component cannot be talked into approving something by a caller passing
	 * the wrong permission name.
	 *
	 * Numeric keypad rather than a text field: this is used one-handed, at a counter, by someone
	 * who was interrupted mid-sale.
	 */
	import { Button } from '@sahl/ui';

	interface Props {
		/** What is being approved, in words a cashier would use. */
		action: string;
		onsubmit: (pin: string) => void;
		oncancel: () => void;
		busy?: boolean;
		/** Set by the caller when the till refused, so the message is the till's, not a guess. */
		error?: string | null;
	}

	let { action, onsubmit, oncancel, busy = false, error = null }: Props = $props();

	let entry = $state('');

	const KEYS = ['1', '2', '3', '4', '5', '6', '7', '8', '9'];

	function press(key: string) {
		// Capped at the domain's maximum so a stuck key cannot build an unbounded string.
		if (entry.length < 8) entry += key;
	}

	function submit() {
		if (!entry) return;
		const pin = entry;
		entry = '';
		onsubmit(pin);
	}
</script>

<!-- Modal semantics without a <dialog>: the till runs one screen at a time and a native dialog's
     top-layer behaviour fights the Tauri webview's focus handling. -->
<div
	class="bg-canvas/90 fixed inset-0 z-50 flex items-center justify-center p-6"
	role="dialog"
	aria-modal="true"
	aria-label="Approval required"
>
	<div class="border-border bg-surface w-full max-w-sm border p-5">
		<p class="label-caps">Approval required</p>
		<p class="text-md mt-1">{action}</p>
		<p class="text-secondary text-text-secondary mt-2">
			A manager enters their own PIN. It is recorded against this action.
		</p>

		<div
			class="border-border bg-surface-sunken mt-4 flex items-center justify-center border"
			style="min-height: var(--scale-touch-target)"
		>
			<!-- Masked, not blanked: a cashier needs to see how many digits landed. -->
			<span class="numeric text-lg tracking-widest" aria-live="polite">
				{'•'.repeat(entry.length) || '····'}
			</span>
		</div>

		{#if error}
			<p class="text-secondary text-danger-text mt-2" role="alert">{error}</p>
		{/if}

		<div class="mt-4 grid grid-cols-3 gap-2">
			{#each KEYS as key (key)}
				<Button variant="secondary" size="lg" onclick={() => press(key)} disabled={busy}>
					{key}
				</Button>
			{/each}
			<Button
				variant="ghost"
				size="lg"
				onclick={() => (entry = entry.slice(0, -1))}
				disabled={busy}
			>
				←
			</Button>
			<Button variant="secondary" size="lg" onclick={() => press('0')} disabled={busy}>0</Button>
			<Button variant="primary" size="lg" onclick={submit} disabled={busy || !entry}>OK</Button>
		</div>

		<div class="mt-3">
			<Button variant="ghost" size="lg" onclick={oncancel} disabled={busy}>Cancel</Button>
		</div>
	</div>
</div>
