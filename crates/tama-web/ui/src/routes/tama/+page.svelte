<script lang="ts">
	import { onMount } from 'svelte';
	import { api } from '$lib/api/client';
	import { connectMetrics, disconnectMetrics, metricsHistory, metricsError } from '$lib/stores/metrics';
	import { formatNumber, formatMib } from '$lib/utils/formatting';
	import type { MetricSample, ModelStatus } from '$lib/types/metrics';
	import SparklineChart from '$lib/components/SparklineChart.svelte';
	import StatCard from '$lib/components/StatCard.svelte';
	import ModelCard from '$lib/components/ModelCard.svelte';
	import type { ModelEntry } from '$lib/types/models';
	import { addToast } from '$lib/stores/toasts';

	// SSE-connected metrics
	let latestMetrics = $derived.by(() => {
		const history = $metricsHistory;
		return history.length > 0 ? history[history.length - 1] : null;
	});

	let hasError = $derived($metricsError);

	// All timestamps for sparkline (from the latest sample's ts_unix_ms)
	let cpuData = $derived<MetricSample[]>($metricsHistory.map((s) => s).reverse());
	let cpuValues = $derived(cpuData.map((s) => s.cpu_usage_pct));
	let cpuTimestamps = $derived(cpuData.map((s) => s.ts_unix_ms));

	let ramValues = $derived(cpuData.map((s) => s.ram_used_mib));
	let ramTimestamps = $derived(cpuData.map((s) => s.ts_unix_ms));

	let gpuValues = $derived(cpuData.map((s) => s.gpu_utilization_pct ?? 0).filter((_, i) => cpuData[i].gpu_utilization_pct !== undefined));
	let gpuTimestamps = $derived(
		cpuData
			.filter((s) => s.gpu_utilization_pct !== undefined)
			.map((s) => s.ts_unix_ms)
	);

	let vramValues = $derived(
		cpuData.map((s) => s.vram?.used_mib ?? 0).filter((_, i) => cpuData[i].vram !== undefined)
	);
	let vramTimestamps = $derived(
		cpuData
			.filter((s) => s.vram !== undefined)
			.map((s) => s.ts_unix_ms)
	);

	let vramMax = $derived(
		cpuData
			.map((s) => s.vram?.total_mib ?? 0)
			.filter((_, i) => cpuData[i].vram !== undefined)
			.reduce((a, b) => Math.max(a, b), 0)
	);

	let tpsValues = $derived(
		cpuData.map((s) => s.tps ?? 0).filter((_, i) => cpuData[i].tps !== undefined)
	);
	let tpsTimestamps = $derived(
		cpuData
			.filter((s) => s.tps !== undefined)
			.map((s) => s.ts_unix_ms)
	);

	let promptTpsValues = $derived(
		cpuData.map((s) => s.prompt_tps ?? 0).filter((_, i) => cpuData[i].prompt_tps !== undefined)
	);
	let promptTpsTimestamps = $derived(
		cpuData
			.filter((s) => s.prompt_tps !== undefined)
			.map((s) => s.ts_unix_ms)
	);

	// Has GPU/VRAM data at all?
	let hasGpuData = $derived(latestMetrics?.gpu_utilization_pct !== undefined);
	let hasVramData = $derived(latestMetrics?.vram !== undefined);

	// Models from latest metrics
	let allModels = $derived<ModelStatus[]>(latestMetrics?.models ?? []);
	let activeModels = $derived(
		allModels.filter((m) => ['ready', 'loading', 'unloading'].includes(m.state))
	);
	let inactiveModels = $derived(
		allModels.filter((m) => !['ready', 'loading', 'unloading'].includes(m.state))
	);

	function modelDisplayName(m: ModelStatus): string {
		return m.display_name || m.api_name || `Model ${m.id}`;
	}

	// Convert ModelStatus to ModelEntry for ModelCard compatibility
	function toModelEntry(m: ModelStatus): ModelEntry {
		return {
			id: m.db_id ?? parseInt(m.id, 10) ?? 0,
			backend: m.backend,
			model: m.api_name ?? null,
			quant: m.quant ?? null,
			enabled: m.state !== 'failed',
			loaded: m.loaded ?? m.state === 'ready',
			state: m.state,
			api_name: m.api_name ?? null,
			display_name: m.display_name ?? null
		};
	}

	async function handleRestart() {
		try {
			const res = await api.post('/system/restart');
			if (!res.ok) {
				const text = await res.text();
				throw new Error(`Failed to restart: ${res.status} ${text}`);
			}
			addToast('Restarting', 'System restart initiated.', 'info');
		} catch (e: any) {
			addToast('Error', `Failed to restart: ${e.message}`, 'error');
		}
	}

	async function handleLoadModel(model: ModelEntry) {
		try {
			const res = await api.post(`/models/${model.id}/load`);
			if (!res.ok) {
				const text = await res.text();
				throw new Error(`Failed to load model: ${res.status} ${text}`);
			}
			addToast('Success', `Model "${model.display_name || model.api_name || String(model.id)}" loading.`, 'success');
		} catch (e: any) {
			addToast('Error', `Failed to load model: ${e.message}`, 'error');
		}
	}

	async function handleUnloadModel(model: ModelEntry) {
		try {
			const res = await api.post(`/models/${model.id}/unload`);
			if (!res.ok) {
				const text = await res.text();
				throw new Error(`Failed to unload model: ${res.status} ${text}`);
			}
			addToast('Success', `Model "${model.display_name || model.api_name || String(model.id)}" unloading.`, 'success');
		} catch (e: any) {
			addToast('Error', `Failed to unload model: ${e.message}`, 'error');
		}
	}

	async function handleRetry() {
		disconnectMetrics();
		connectMetrics();
	}

	onMount(() => {
		connectMetrics();
		return () => disconnectMetrics();
	});

	// Status badge
	let statusBadge = $derived(
		hasError
			? 'badge-danger'
			: latestMetrics
				? 'badge-success'
				: 'badge-info'
	);
	let statusText = $derived(
		hasError ? 'Disconnected' : latestMetrics ? 'Live' : 'Connecting...'
	);
</script>

<div class="page">
	{#if hasError}
		<!-- Error state -->
		<div class="page-header">
			<h1>&#127968; Dashboard</h1>
		</div>
		<div class="card flex flex-col items-center justify-center py-12 gap-4">
			<div class="flex items-center gap-2">
				<span class="badge badge-danger">Disconnected</span>
			</div>
			<p class="text-text-secondary">Failed to load metrics stream</p>
			<button class="btn btn-primary" onclick={handleRetry}>
				&#8633; Retry
			</button>
		</div>
	{:else}
		<!-- Page header -->
		<div class="page-header">
			<div class="flex items-center gap-3">
				<h1>&#127968; Dashboard</h1>
				<span class="badge {statusBadge}">{statusText}</span>
			</div>
			<div class="page-header-actions">
				<button class="btn btn-danger" onclick={handleRestart}>
					&#8635; Restart
				</button>
				<button class="btn btn-primary" disabled>
					+ Pull Model
				</button>
			</div>
		</div>

		{#if latestMetrics}
			<!-- System stats grid -->
			<div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-3 mb-6">
				<!-- CPU -->
				<StatCard title="CPU" value={`${latestMetrics.cpu_usage_pct.toFixed(1)}%`}>
					{#snippet sparkline()}
						<SparklineChart
							data={cpuValues}
							maxValue={100}
							color="var(--color-accent-blue)"
							timestamps={cpuTimestamps}
							unitLabel="%"
						/>
					{/snippet}
				</StatCard>

				<!-- Memory -->
				<StatCard
					title="Memory"
					value={formatMib(latestMetrics.ram_used_mib)}
					secondary={`${formatMib(latestMetrics.ram_total_mib)} total`}
				>
					{#snippet sparkline()}
						<SparklineChart
							data={ramValues}
							maxValue={latestMetrics.ram_total_mib}
							color="var(--color-accent-cyan)"
							timestamps={ramTimestamps}
							unitLabel=" MiB"
						/>
					{/snippet}
				</StatCard>

				<!-- GPU (conditional) -->
				{#if hasGpuData}
					<StatCard title="GPU" value={`${latestMetrics.gpu_utilization_pct ?? 0}%`}>
						{#snippet sparkline()}
							<SparklineChart
								data={gpuValues}
								maxValue={100}
								color="var(--color-accent-green)"
								timestamps={gpuTimestamps}
								unitLabel="%"
							/>
						{/snippet}
					</StatCard>
				{/if}

				<!-- VRAM (conditional) -->
				{#if hasVramData}
					<StatCard
						title="VRAM"
						value={formatMib(latestMetrics.vram?.used_mib ?? 0)}
						secondary={latestMetrics.vram ? `${formatMib(latestMetrics.vram.total_mib)} total` : ''}
					>
						{#snippet sparkline()}
							<SparklineChart
								data={vramValues}
								maxValue={vramMax || 1}
								color="var(--color-accent-purple)"
								timestamps={vramTimestamps}
								unitLabel=" MiB"
							/>
						{/snippet}
					</StatCard>
				{/if}
			</div>

			<!-- Inference stats grid -->
			<div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-3 mb-6">
				<StatCard
					title="Processing Speed"
					value={latestMetrics.prompt_tps !== undefined ? `${latestMetrics.prompt_tps.toFixed(1)} t/s` : '—'}
				>
					{#snippet sparkline()}
						{#if promptTpsValues.length > 0}
							<SparklineChart
								data={promptTpsValues}
								maxValue={Math.max(...promptTpsValues, 1) * 1.2}
								color="var(--color-accent-orange)"
								timestamps={promptTpsTimestamps}
								unitLabel=" t/s"
							/>
						{/if}
					{/snippet}
				</StatCard>

				<StatCard
					title="Gen Speed"
					value={latestMetrics.tps !== undefined ? `${latestMetrics.tps.toFixed(1)} t/s` : '—'}
				>
					{#snippet sparkline()}
						{#if tpsValues.length > 0}
							<SparklineChart
								data={tpsValues}
								maxValue={Math.max(...tpsValues, 1) * 1.2}
								color="var(--color-accent-green)"
								timestamps={tpsTimestamps}
								unitLabel=" t/s"
							/>
						{/if}
					{/snippet}
				</StatCard>

				<StatCard
					title="Cache Hits"
					value={latestMetrics.cache_hit_pct !== undefined ? `${latestMetrics.cache_hit_pct.toFixed(1)}%` : '—'}
				/>

				<StatCard
					title="Spec Accept"
					value={latestMetrics.spec_accept_pct !== undefined ? `${latestMetrics.spec_accept_pct.toFixed(1)}%` : '—'}
					secondary={latestMetrics.spec_decoding_active ? 'Active' : 'Inactive'}
				/>
			</div>

			<!-- Models section -->
			{#if allModels.length > 0}
				<!-- Active Models -->
				{#if activeModels.length > 0}
					<h2 class="text-lg font-semibold text-text-primary mb-3">Active Models</h2>
					<div class="flex flex-col gap-3 mb-6">
						{#each activeModels as model (model.id)}
							<ModelCard
								model={toModelEntry(model)}
								onLoad={handleLoadModel}
								onUnload={handleUnloadModel}
								logSource={model.backend}
							/>
						{/each}
					</div>
				{/if}

				<!-- Inactive Models -->
				{#if inactiveModels.length > 0}
					<h2 class="text-lg font-semibold text-text-primary mb-3">Inactive Models</h2>
					<div class="flex flex-col gap-3 mb-6">
						{#each inactiveModels as model (model.id)}
							<ModelCard
								model={toModelEntry(model)}
								onLoad={handleLoadModel}
								onUnload={handleUnloadModel}
								logSource={model.backend}
							/>
						{/each}
					</div>
				{/if}
			{:else}
				<div class="card flex flex-col items-center justify-center py-12 gap-3">
					<p class="text-muted">No models in metrics stream</p>
				</div>
			{/if}
		{:else}
			<!-- Loading state -->
			<div class="card flex items-center justify-center gap-3 py-12">
				<div class="h-5 w-5 animate-spin rounded-full border-2 border-text-muted border-t-accent-blue"></div>
				<span class="text-muted">Connecting to metrics stream...</span>
			</div>
		{/if}
	{/if}
</div>
