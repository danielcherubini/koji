<script lang="ts">
	import { fetchUpdates, checkUpdates, updateBackend, applyModelUpdate, checkBackend } from '$lib/api/updates';
	import type { UpdateCheckDto, UpdatesListResponse } from '$lib/types/updates';
	import { onMount } from 'svelte';
	import { addToast } from '$lib/stores/toasts';

	let data = $state<UpdatesListResponse>({ backends: [], models: [] });
	let checking = $state(false);
	let lastChecked = $state<number | null>(null);
	let error = $state<string | null>(null);

	// Model quants expansion & selection
	let expandedModels = $state<Record<string, boolean>>({});
	let modelSelections = $state<Record<string, string[]>>({});
	let modelUpdateBusy = $state<string | null>(null);

	async function loadUpdates() {
		try {
			data = await fetchUpdates();
			const allItems = [...data.backends, ...data.models];
			lastChecked = allItems.reduce((max, item) => Math.max(max, item.checked_at), 0) || null;
		} catch (e: any) {
			error = e.message || 'Failed to load updates.';
		}
	}

	onMount(() => {
		loadUpdates();
	});

	async function handleCheckNow() {
		checking = true;
		error = null;
		try {
			await checkUpdates();
			// Wait a bit then refresh
			await new Promise((r) => setTimeout(r, 2000));
			await loadUpdates();
		} catch (e: any) {
			error = e.message || 'Failed to trigger check.';
		}
		checking = false;
	}

	async function handleUpdateBackend(name: string) {
		try {
			const result = await updateBackend(name);
			addToast('Update Started', `Backend "${name}" update job started: ${result.job_id}`, 'success');
			// Refresh after a delay
			await new Promise((r) => setTimeout(r, 3000));
			await loadUpdates();
		} catch (e: any) {
			error = e.message || 'Failed to update backend.';
		}
	}

	function shortSha(hash: string | null | undefined): string {
		if (!hash) return '—';
		return hash.slice(0, 8);
	}

	function toggleExpand(modelId: string) {
		expandedModels = { ...expandedModels, [modelId]: !expandedModels[modelId] };
	}

	function isExpanded(modelId: string): boolean {
		return !!expandedModels[modelId];
	}

	function getQuants(model: UpdateCheckDto): { quant_name: string; filename: string; current_hash: string | null; latest_hash: string | null; update_available: boolean }[] {
		const quants = model.details_json?.quants;
		if (!Array.isArray(quants)) return [];
		return quants.map((q: any) => ({
			quant_name: q.quant_name || q.filename || 'unknown',
			filename: q.filename || '',
			current_hash: q.current_hash || null,
			latest_hash: q.latest_hash || null,
			update_available: !!q.update_available
		}));
	}

	function hasUpdates(model: UpdateCheckDto): boolean {
		return getQuants(model).some((q) => q.update_available);
	}

	function toggleQuantSelection(modelId: string, quantKey: string) {
		const current = modelSelections[modelId] || [];
		if (current.includes(quantKey)) {
			modelSelections = {
				...modelSelections,
				[modelId]: current.filter((k) => k !== quantKey)
			};
		} else {
			modelSelections = {
				...modelSelections,
				[modelId]: [...current, quantKey]
			};
		}
	}

	function selectAllQuants(modelId: string, quants: { quant_name: string; update_available: boolean }[]) {
		modelSelections = {
			...modelSelections,
			[modelId]: quants.filter((q) => q.update_available).map((q) => q.quant_name)
		};
	}

	function selectedCount(modelId: string): number {
		return (modelSelections[modelId] || []).length;
	}

	async function handleApplyModelUpdate(modelId: string) {
		const selected = modelSelections[modelId] || [];
		if (selected.length === 0) return;
		modelUpdateBusy = modelId;
		try {
			await applyModelUpdate(modelId, selected);
			// Clear selections
			const newSelections = { ...modelSelections };
			delete newSelections[modelId];
			modelSelections = newSelections;
			addToast('Update Started', `Model update started with ${selected.length} quant(s).`, 'success');
			// Refresh
			await new Promise((r) => setTimeout(r, 2000));
			await loadUpdates();
		} catch (e: any) {
			error = e.message || 'Failed to apply model update.';
		}
		modelUpdateBusy = null;
	}

	async function handleCheckBackend(name: string) {
		try {
			await checkBackend(name);
			await loadUpdates();
		} catch {
			// non-critical
		}
	}

	function formatTimestamp(ts: number | null): string {
		if (!ts) return 'Never';
		return new Date(ts * 1000).toLocaleString();
	}

	let displayName = $derived(
		(item: UpdateCheckDto) => item.display_name || item.repo_id || item.item_id
	);
</script>

<div class="page">
	<div class="page-header">
		<h1>&#128260; Updates Center</h1>
		<div class="page-header-actions">
			<span class="text-sm text-text-muted">Last checked: {formatTimestamp(lastChecked)}</span>
			<button class="btn btn-primary" onclick={handleCheckNow} disabled={checking}>
				{checking ? 'Checking...' : 'Check Now'}
			</button>
		</div>
	</div>

	{#if error}
		<div class="mb-4 rounded-md bg-accent-red/20 px-4 py-3 text-sm text-accent-red">{error}</div>
	{/if}

	<!-- Backends Section -->
	<section class="mb-8">
		<h2 class="mb-3 text-lg font-semibold text-text-primary">Backends</h2>
		<div class="flex flex-col gap-3">
			{#each data.backends as backend (backend.item_id)}
				<div class="card flex items-center justify-between">
					<div class="flex items-center gap-3">
						<span class="font-medium text-text-primary">{backend.item_id}</span>
						{#if backend.variant}
							<span class="text-sm text-text-muted">{backend.variant}</span>
						{/if}
						<span class="text-sm text-text-secondary">{backend.current_version || '—'}</span>
						{#if backend.update_available}
							<span class="badge badge-warning">&rarr; {backend.latest_version}</span>
						{:else}
							<span class="badge badge-success">&#10003; Up to date</span>
						{/if}
					</div>
					<div class="flex items-center gap-2">
						{#if backend.update_available}
							<button class="btn btn-secondary btn-sm" onclick={() => handleUpdateBackend(backend.item_id)}>
								Update
							</button>
						{/if}
						<button class="btn btn-secondary btn-sm" onclick={() => handleCheckBackend(backend.item_id)}>
							Refresh
						</button>
					</div>
				</div>
			{:else}
				<div class="card flex items-center justify-center py-8">
					<span class="text-muted">No backends found. Add a backend to see update status.</span>
				</div>
			{/each}
		</div>
	</section>

	<!-- Models Section -->
	<section class="mb-8">
		<h2 class="mb-3 text-lg font-semibold text-text-primary">Models</h2>
		<div class="flex flex-col gap-3">
			{#each data.models as model (model.item_id)}
				{@const quants = getQuants(model)}
				{@const modelHasUpdates = hasUpdates(model)}
				<div class="card">
					<div class="flex items-center justify-between">
						<div class="flex items-center gap-3">
							<button
								class="text-text-muted hover:text-text-primary text-xs transition-colors"
								onclick={() => toggleExpand(model.item_id)}
							>
								{isExpanded(model.item_id) ? '&#9660;' : '&#9654;'}
							</button>
							<span class="font-medium text-text-primary">{displayName(model)}</span>
							<span class="text-sm text-text-secondary">{shortSha(model.current_version)}</span>
							{#if modelHasUpdates}
								<span class="badge badge-warning">&rarr; {shortSha(model.latest_version)}</span>
							{:else}
								<span class="badge badge-success">&#10003; Up to date</span>
							{/if}
						</div>
					</div>

					{#if isExpanded(model.item_id)}
						<div class="mt-3 ml-6">
							<button
								class="btn btn-secondary btn-sm mb-2"
								onclick={() => selectAllQuants(model.item_id, quants)}
							>
								Select All
							</button>
							{#each quants as quant (quant.quant_name)}
								<label class="flex items-center gap-2 py-1 text-sm">
									<input
										type="checkbox"
										class="rounded border-border-default bg-bg-primary text-accent-blue"
										disabled={!quant.update_available}
										checked={Boolean((modelSelections[model.item_id] || []).includes(quant.quant_name))}
										onchange={() => toggleQuantSelection(model.item_id, quant.quant_name)}
									/>
									<span class="font-medium text-text-primary">{quant.quant_name}</span>
									<span class="text-text-muted text-xs">{shortSha(quant.current_hash)}</span>
									<span class="text-text-muted">&rarr;</span>
									<span class="text-text-muted text-xs">{shortSha(quant.latest_hash)}</span>
									{#if quant.update_available}
										<span class="badge badge-warning text-xs">Update</span>
									{:else}
										<span class="badge badge-success text-xs">Up to date</span>
									{/if}
								</label>
							{/each}
							{#if selectedCount(model.item_id) > 0}
								<button
									class="btn btn-primary btn-sm mt-2"
									disabled={modelUpdateBusy === model.item_id}
									onclick={() => handleApplyModelUpdate(model.item_id)}
								>
									{modelUpdateBusy === model.item_id ? 'Updating...' : `Update Selected (${selectedCount(model.item_id)})`}
								</button>
							{/if}
						</div>
					{/if}
				</div>
			{:else}
				<div class="card flex items-center justify-center py-8">
					<span class="text-muted">No models found. Download a model to see update status.</span>
				</div>
			{/each}
		</div>
	</section>
</div>
