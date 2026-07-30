<script lang="ts">
	/**
	 * Put a named person at the till.
	 *
	 * Two steps rather than one, because a shared counter has several people and none of them
	 * should have to type an id: pick a face from the list, then your own PIN. The PIN goes
	 * straight to Rust and is cleared the moment the call returns.
	 *
	 * Nothing here decides who may do what. It establishes *who*, and the till decides the rest —
	 * a screen that also carried authority would be a second answer to the same question.
	 */
	import { Button } from '@sahl/ui';
	import { asTillError, till, type StaffView } from '$lib/till';

	interface Props {
		/** Called with the person now at the till. */
		onsignedin: (who: StaffView) => void;
	}

	let { onsignedin }: Props = $props();

	let staff = $state<StaffView[]>([]);
	let chosen = $state<StaffView | null>(null);
	let entry = $state('');
	let busy = $state(false);
	let error = $state<string | null>(null);

	const KEYS = ['1', '2', '3', '4', '5', '6', '7', '8', '9'];

	$effect(() => {
		void (async () => {
			try {
				staff = await till.staffList();
			} catch (thrown) {
				error = asTillError(thrown).message;
			}
		})();
	});

	function press(key: string) {
		// Capped at the domain's maximum so a stuck key cannot build an unbounded string.
		if (entry.length < 8) entry += key;
	}

	function submit() {
		const who = chosen;
		if (!who || !entry) return;
		const pin = entry;
		entry = '';

		void (async () => {
			busy = true;
			error = null;
			try {
				onsignedin(await till.signIn(who.id, pin));
			} catch (thrown) {
				// The till's own wording. "Unknown" and "wrong PIN" are deliberately the same
				// message there, and second-guessing it here would leak which half was wrong.
				error = asTillError(thrown).message;
			} finally {
				busy = false;
			}
		})();
	}
</script>

<div class="flex min-h-dvh items-center justify-center p-6">
	<div class="border-border bg-surface w-full max-w-sm border p-5">
		{#if !chosen}
			<p class="label-caps">Who is at the till</p>

			{#if staff.length === 0}
				<p class="text-secondary text-text-muted mt-3">
					Nobody is enrolled yet. Add the first person in Staff — that one is allowed without
					approval, because there is nobody yet to approve it.
				</p>
				<a class="text-secondary text-primary mt-3 inline-block" href="/staff">Go to Staff</a>
			{:else}
				<div class="mt-3 flex flex-col gap-2">
					{#each staff as member (member.id)}
						<Button
							variant="secondary"
							size="lg"
							block
							onclick={() => {
								chosen = member;
								error = null;
							}}
						>
							{member.name}
						</Button>
					{/each}
				</div>
			{/if}
		{:else}
			<p class="label-caps">PIN</p>
			<p class="text-md mt-1">{chosen.name}</p>

			<div
				class="border-border bg-surface-sunken mt-4 flex items-center justify-center border"
				style="min-height: var(--scale-touch-target)"
			>
				<!-- Masked, not blanked: somebody needs to see how many digits landed. -->
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
					variant="secondary"
					size="lg"
					onclick={() => {
						chosen = null;
						entry = '';
					}}
					disabled={busy}
				>
					Back
				</Button>
				<Button variant="secondary" size="lg" onclick={() => press('0')} disabled={busy}>0</Button>
				<Button variant="primary" size="lg" onclick={submit} disabled={busy || !entry}>
					Sign in
				</Button>
			</div>
		{/if}
	</div>
</div>
