<script lang="ts">
	import type { ModelEntry } from '$lib/types/models';

	interface Props {
		model: ModelEntry;
		showLoadUnload?: boolean;
		logSource?: string | null;
		onLoad?: (model: ModelEntry) => Promise<void>;
		onUnload?: (model: ModelEntry) => Promise<void>;
	}

	let {
		model,
		showLoadUnload = true,
		logSource = null,
		onLoad,
		onUnload
	}: Props = $props();

	let loading = $state(false);

	// Effective state: if state is empty, derive from loaded flag
	let effectiveState = $derived(
		model.state || (model.loaded ? 'ready' : 'idle')
	);

	// Display name: prefer display_name, then api_name, then id
	let displayName = $derived(
		model.display_name || model.api_name || `Model ${model.id}`
	);

	function getStateLabel(state: string): string {
		switch (state) {
			case 'ready': return 'Loaded';
			case 'loading': return 'Loading';
			case 'unloading': return 'Unloading';
			case 'failed': return 'Failed';
			default: return 'Idle';
		}
	}

	function getStateBadgeClass(state: string): string {
		switch (state) {
			case 'ready': return 'badge-success';
			case 'loading': return 'badge-info';
			case 'unloading': return 'badge-warning';
			case 'failed': return 'badge-danger';
			default: return '';
		}
	}

	function getAccentColor(state: string): string {
		switch (state) {
			case 'ready': return 'border-accent-green';
			case 'loading': return 'border-accent-blue';
			case 'unloading': return 'border-accent-yellow';
			case 'failed': return 'border-accent-red';
			default: return 'border-text-muted';
		}
	}

	function getLoadButtonClass(state: string): string {
		if (state === 'idle' || state === 'failed') {
			return 'bg-accent-green text-white hover:bg-accent-green/80';
		}
		return 'bg-bg-tertiary text-text-primary';
	}

	async function handleLoad() {
		if (!onLoad) return;
		loading = true;
		try {
			await onLoad(model);
		} finally {
			loading = false;
		}
	}

	async function handleUnload() {
		if (!onUnload) return;
		loading = true;
		try {
			await onUnload(model);
		} finally {
			loading = false;
		}
	}
</script>

<div class="card flex flex-col gap-2" class:border-l-4={true} class:{getAccentColor(effectiveState)}={true}>
	<!-- Line 1: Name, badges, actions -->
	<div class="flex items-center justify-between">
		<div class="flex items-center gap-2 min-w-0">
			<!-- Enabled indicator -->
			<span
				class="inline-block h-2 w-2 rounded-full shrink-0"
				class:bg-accent-green={model.enabled}
				class:bg-text-muted={!model.enabled}
			></span>

			<!-- Model name -->
			<span class="font-medium text-text-primary truncate">{displayName}</span>

			<!-- State badge -->
			<span class="badge {getStateBadgeClass(effectiveState)} shrink-0">
				{getStateLabel(effectiveState)}
			</span>

			<!-- Enabled/Disabled badge -->
			<span class="badge shrink-0" class:badge-success={model.enabled} class:badge-danger={!model.enabled}>
				{model.enabled ? 'Enabled' : 'Disabled'}
			</span>
		</div>

		<div class="flex items-center gap-1 shrink-0">
			<!-- Load/Unload button -->
			{#if showLoadUnload}
				{#if effectiveState === 'ready'}
					<button
						class="btn btn-danger btn-sm"
						disabled={loading}
						onclick={handleUnload}
					>
						{loading ? 'Unloading...' : 'Unload'}
					</button>
				{:else if effectiveState === 'loading'}
					<button class="btn btn-secondary btn-sm" disabled>
						Loading...
					</button>
				{:else if effectiveState === 'unloading'}
					<button class="btn btn-secondary btn-sm" disabled>
						Unloading...
					</button>
				{:else}
					<button
						class="btn btn-sm {getLoadButtonClass(effectiveState)}"
						disabled={loading || effectiveState !== 'idle' && effectiveState !== 'failed'}
						onclick={handleLoad}
					>
						{loading ? 'Loading...' : 'Load'}
					</button>
				{/if}
			{/if}

			<!-- Edit link -->
			<a
				href="/tama/model/{model.id}/edit"
				class="btn btn-secondary btn-sm"
				title="Edit"
			>&#9998;</a>

			<!-- Logs link -->
			{#if logSource}
				<a
					href="/tama/logs?source={logSource}"
					class="btn btn-secondary btn-sm"
					title="Logs"
				>&#128221;</a>
			{/if}
		</div>
	</div>

	<!-- Line 2: Quant, Backend -->
	<div class="flex items-center gap-2">
		{#if model.quant}
			<span class="badge badge-info">
				{model.quant}
			</span>
		{/if}
		{#if model.backend}
			<span class="badge badge-info">
				{model.backend}
			</span>
		{/if}
	</div>
</div>
