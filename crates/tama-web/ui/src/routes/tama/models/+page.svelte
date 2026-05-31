<script lang="ts">
	import { listModels, loadModel, unloadModel, refreshAllModels } from '$lib/api/models';
	import type { ModelEntry } from '$lib/types/models';
	import ModelCard from '$lib/components/ModelCard.svelte';
	import { addToast } from '$lib/stores/toasts';

	let { data } = $props();

	let models = $state<ModelEntry[]>(data.models ?? []);
	let checking = $state(false);
	let checkError = $state<string | null>(null);
	let checkSuccess = $state(0);

	async function handleLoad(model: ModelEntry) {
		try {
			await loadModel(model.id);
			models = models.map((m) =>
				m.id === model.id ? { ...m, state: 'loading' } : m
			);
			addToast('Success', `Model "${model.display_name || model.api_name || model.id}" loading.`, 'success');
			// Refresh after a short delay to pick up state change
			await new Promise((r) => setTimeout(r, 1500));
			await refreshModelList();
		} catch (e: any) {
			addToast('Error', `Failed to load model: ${e.message}`, 'error');
		}
	}

	async function handleUnload(model: ModelEntry) {
		try {
			await unloadModel(model.id);
			models = models.map((m) =>
				m.id === model.id ? { ...m, state: 'unloading' } : m
			);
			addToast('Success', `Model "${model.display_name || model.api_name || model.id}" unloading.`, 'success');
			await new Promise((r) => setTimeout(r, 1500));
			await refreshModelList();
		} catch (e: any) {
			addToast('Error', `Failed to unload model: ${e.message}`, 'error');
		}
	}

	async function handleCheckAll() {
		checking = true;
		checkError = null;
		checkSuccess = 0;
		try {
			await refreshAllModels();
			await new Promise((r) => setTimeout(r, 500));
			await refreshModelList();
			checkSuccess = models.length;
			addToast('Success', `Checked ${models.length} model(s) for updates.`, 'success');
		} catch (e: any) {
			checkError = e.message || 'Failed to check for updates.';
			addToast('Error', `Failed to check for updates: ${checkError}`, 'error');
		}
		checking = false;
	}

	async function refreshModelList() {
		try {
			const data = await listModels();
			models = data.models || [];
		} catch (e: any) {
			console.warn('Failed to refresh model list:', e);
		}
	}
</script>

<div class="page">
	<div class="page-header">
		<h1>&#129504; Models</h1>
		<div class="page-header-actions">
			<button class="btn btn-secondary" onclick={handleCheckAll} disabled={checking}>
				{checking ? 'Checking...' : 'Check all for updates'}
			</button>
			<button class="btn btn-primary" disabled>
				+ Pull Model
			</button>
		</div>
	</div>

	{#if checkError}
		<div class="mb-4 rounded-md bg-accent-red/20 px-4 py-3 text-sm text-accent-red">
			{checkError}
		</div>
	{:else if checkSuccess > 0}
		<div class="mb-4 rounded-md bg-accent-green/20 px-4 py-3 text-sm text-accent-green">
			&#10003; Checked {checkSuccess} model(s) for updates.
		</div>
	{/if}

	{#if models.length === 0}
		<div class="card flex flex-col items-center justify-center py-12 gap-3">
			<p class="text-muted">No models configured yet</p>
			<button class="btn btn-primary" disabled>+ Pull a Model</button>
		</div>
	{:else}
		<div class="flex flex-col gap-3">
			{#each models as model (model.id)}
				<ModelCard
					{model}
					onLoad={handleLoad}
					onUnload={handleUnload}
					logSource={model.backend}
				/>
			{/each}
		</div>
	{/if}
</div>
