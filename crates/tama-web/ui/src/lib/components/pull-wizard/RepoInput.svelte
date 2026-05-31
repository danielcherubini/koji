<script lang="ts">
	interface Props {
		initialRepo: string;
		onLoad: (repoId: string) => void;
	}

	let { initialRepo, onLoad }: Props = $props();

	let repoId = $state(initialRepo);
	let error = $state<string | null>(null);

	function handleLoad() {
		const trimmed = repoId.trim();
		if (!trimmed) {
			error = 'Please enter a HuggingFace repo ID.';
			return;
		}
		error = null;
		onLoad(trimmed);
	}

	function handleKeydown(e: KeyboardEvent) {
		if (e.key === 'Enter') {
			handleLoad();
		}
	}
</script>

<div class="space-y-3">
	<div>
		<label class="mb-1 block text-sm font-medium text-text-primary" for="repo-id">HuggingFace Repo ID</label>
		<input
			id="repo-id"
			type="text"
			class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
			placeholder="e.g.bartowski/Llama-3.1-8B-Instruct-Q4_K_M-IQ1_M-hf"
			bind:value={repoId}
			onkeydown={handleKeydown}
			autofocus
		/>
		<p class="mt-1 text-xs text-text-muted">Format: author/repo-name</p>
	</div>

	{#if error}
		<div class="rounded-md bg-accent-red/20 px-3 py-2 text-sm text-accent-red">{error}</div>
	{/if}

	<div class="flex justify-end">
		<button
			class="btn btn-primary"
			onclick={handleLoad}
		>
			Load
		</button>
	</div>
</div>
