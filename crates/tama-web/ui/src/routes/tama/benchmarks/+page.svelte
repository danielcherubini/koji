<script lang="ts">
	import {
		runBenchmark,
		runSpecBenchmark,
		runMtpBenchmark,
		deleteBenchmark,
		listBenchmarkHistory
	} from '$lib/api/benchmarks';
	import type { ModelEntry } from '$lib/types/models';
	import type {
		HistoryEntry,
		BenchmarkPreset,
		SpecType,
		SpecPreset
	} from '$lib/types/benchmarks';
	import {
		BENCHMARK_PRESETS,
		SPEC_PRESETS,
		DEFAULT_DRAFT_MAX_VALUES,
		DEFAULT_NGL,
		DEFAULT_DRAFT_NGL,
		DEFAULT_CONTEXT_SIZE,
		parseNumList,
		formatNumList,
		formatBenchmarkTime,
		getHistoryDisplayName
	} from '$lib/types/benchmarks';
	import JobLogPanel from '$lib/components/JobLogPanel.svelte';
	import { addToast } from '$lib/stores/toasts';

	let { data } = $props();

	let models = $state<ModelEntry[]>(data.models ?? []);
	let history = $state<HistoryEntry[]>(data.history ?? []);

	// Active tab
	let activeTab = $state<'standard' | 'mtp' | 'spec' | 'history'>('standard');

	// Job tracking
	let activeJobId = $state<string | null>(null);
	let jobTitle = $state('Benchmark');
	let benchmarkResult = $state<string | null>(null);
	let benchmarkResultParsed = $state<any>(null);

	// Error
	let error = $state<string | null>(null);
	let running = $state(false);

	// ── Standard benchmark form ──────────────────────────────────────
	let stdModelId = $state<string>('');
	let stdQuant = $state<string>('');
	let stdPresetIdx = $state(0);
	let stdPpSizes = $state('2048');
	let stdTgSizes = $state('128');
	let stdRuns = $state(3);
	let stdThreads = $state('');
	let stdNglRange = $state('99');
	let stdBatchSizes = $state('');
	let stdUbatchSizes = $state('');
	let stdKvCacheType = $state('');
	let stdDepth = $state('');
	let stdFlashAttn = $state(true);

	// ── MTP benchmark form ───────────────────────────────────────────
	let mtpModelId = $state<string>('');
	let mtpQuant = $state<string>('');
	let mtpDraftMax = $state(formatNumList(DEFAULT_DRAFT_MAX_VALUES));
	let mtpNgl = $state(String(DEFAULT_NGL));
	let mtpDraftNgl = $state(String(DEFAULT_DRAFT_NGL));
	let mtpFlashAttn = $state(true);
	let mtpContextSize = $state(String(DEFAULT_CONTEXT_SIZE));

	// ── Spec benchmark form ──────────────────────────────────────────
	let specModelId = $state<string>('');
	let specQuant = $state<string>('');
	let specType = $state<SpecType>('ngram');
	let specPresetIdx = $state(0);
	let specDraftMax = $state('256');
	let specNgramN = $state('16');
	let specNgramM = $state('12');
	let specNgramMin = $state('');
	let specNgramMax = $state('48');
	let specNgramMinHits = $state(1);
	let specGenTokens = $state(256);
	let specRuns = $state(3);
	let specNgl = $state('99');
	let specFlashAttn = $state(true);

	// Model options for dropdowns
	let modelOptions = $derived(
		models.map((m) => ({
			id: String(m.id),
			label: m.display_name || m.api_name || `Model ${m.id}`,
			backend: m.backend || ''
		}))
	);

	function selectModel(modelId: string, tab: 'std' | 'mtp' | 'spec') {
		const id = String(modelId);
		if (tab === 'std') stdModelId = id;
		else if (tab === 'mtp') mtpModelId = id;
		else specModelId = id;
	}

	function applyPreset(preset: BenchmarkPreset) {
		stdPpSizes = formatNumList(preset.pp_sizes);
		stdTgSizes = formatNumList(preset.tg_sizes);
		stdRuns = preset.runs;
		stdNglRange = preset.ngl_range ?? '';
		stdBatchSizes = formatNumList(preset.batch_sizes);
		stdUbatchSizes = formatNumList(preset.ubatch_sizes);
		stdKvCacheType = preset.kv_cache_type ?? '';
		stdDepth = formatNumList(preset.depth);
		stdFlashAttn = preset.flash_attn ?? true;
	}

	function applySpecPreset(preset: SpecPreset) {
		specDraftMax = formatNumList(preset.draft_max_values);
		specNgramN = preset.ngram_n_values;
		specNgramM = preset.ngram_m_values;
		specNgramMax = preset.ngram_max_values;
	}

	// ── Run handlers ─────────────────────────────────────────────────

	async function handleRunStandard() {
		error = null;
		running = true;
		benchmarkResult = null;
		benchmarkResultParsed = null;

		try {
			const config = {
				model_id: stdModelId,
				pp_sizes: parseNumList(stdPpSizes),
				tg_sizes: parseNumList(stdTgSizes),
				runs: stdRuns,
				warmup: 0,
				threads: stdThreads ? parseNumList(stdThreads) : undefined,
				ngl_range: stdNglRange || undefined,
				batch_sizes: stdBatchSizes ? parseNumList(stdBatchSizes) : [],
				ubatch_sizes: stdUbatchSizes ? parseNumList(stdUbatchSizes) : [],
				kv_cache_type: stdKvCacheType || undefined,
				depth: stdDepth ? parseNumList(stdDepth) : [],
				flash_attn: stdFlashAttn,
				benchmark_type: BENCHMARK_PRESETS[stdPresetIdx]?.label.split('. ')[1]?.toLowerCase().replace(/\s+/g, '_')
			};

			const result = await runBenchmark(config);
			activeJobId = result.job_id;
			jobTitle = 'Standard Benchmark';
			addToast('Benchmark Started', `Job: ${result.job_id}`, 'success');
		} catch (e: any) {
			const msg = e.message || 'Failed to run benchmark.';
			error = msg;
			addToast('Error', msg, 'error');
			running = false;
		}
	}

	async function handleRunMtp() {
		error = null;
		running = true;
		benchmarkResult = null;
		benchmarkResultParsed = null;

		try {
			const config = {
				model_id: mtpModelId,
				draft_max_values: parseNumList(mtpDraftMax),
				ngl: mtpNgl ? parseInt(mtpNgl, 10) : undefined,
				draft_ngl: mtpDraftNgl ? parseInt(mtpDraftNgl, 10) : undefined,
				flash_attn: mtpFlashAttn,
				context_size: mtpContextSize ? parseInt(mtpContextSize, 10) : undefined,
				benchmark_type: 'mtp_sweep'
			};

			const result = await runMtpBenchmark(config);
			activeJobId = result.job_id;
			jobTitle = 'MTP Benchmark';
			addToast('MTP Benchmark Started', `Job: ${result.job_id}`, 'success');
		} catch (e: any) {
			const msg = e.message || 'Failed to run MTP benchmark.';
			error = msg;
			addToast('Error', msg, 'error');
			running = false;
		}
	}

	async function handleRunSpec() {
		error = null;
		running = true;
		benchmarkResult = null;
		benchmarkResultParsed = null;

		try {
			const config = {
				model_id: specModelId,
				spec_types: [specType],
				draft_max_values: parseNumList(specDraftMax),
				ngram_n_values: parseNumList(specNgramN),
				ngram_m_values: parseNumList(specNgramM),
				ngram_min_values: parseNumList(specNgramMin),
				ngram_max_values: parseNumList(specNgramMax),
				ngram_min_hits: specNgramMinHits,
				gen_tokens: specGenTokens,
				runs: specRuns,
				ngl: specNgl ? parseInt(specNgl, 10) : undefined,
				flash_attn: specFlashAttn,
				benchmark_type: specType === 'ngram' ? 'spec_scan' : 'spec_sweep'
			};

			const result = await runSpecBenchmark(config);
			activeJobId = result.job_id;
			jobTitle = 'Spec Decoding Benchmark';
			addToast('Spec Benchmark Started', `Job: ${result.job_id}`, 'success');
		} catch (e: any) {
			const msg = e.message || 'Failed to run spec benchmark.';
			error = msg;
			addToast('Error', msg, 'error');
			running = false;
		}
	}

	async function handleDeleteHistory(id: number) {
		if (!confirm('Delete this benchmark result?')) return;
		try {
			await deleteBenchmark(id);
			history = history.filter((h) => h.id !== id);
			addToast('Deleted', 'Benchmark result deleted.', 'info');
		} catch (e: any) {
			addToast('Error', `Failed to delete: ${e.message}`, 'error');
		}
	}

	function handleBenchmarkResult(results: string) {
		benchmarkResult = results;
		try {
			benchmarkResultParsed = JSON.parse(results);
		} catch {
			benchmarkResultParsed = null;
		}
	}

	function handleBenchmarkDone() {
		running = false;
		listBenchmarkHistory()
			.then((h) => (history = h))
			.catch(() => {});
	}

	function getTabClass(tab: string): string {
		const isActive = activeTab === tab;
		return isActive
			? 'border-accent-blue text-accent-blue'
			: 'border-transparent text-text-muted hover:text-text-primary';
	}

	// ── Result display helpers ────────────────────────────────────────

	function getResultSummary(): string {
		if (!benchmarkResultParsed) return '';
		const summaries = benchmarkResultParsed.summaries || benchmarkResultParsed.entries || [];
		if (summaries.length === 0) return 'No results yet.';
		return `${summaries.length} result(s) available`;
	}
</script>

<div class="page">
	<div class="page-header">
		<h1>&#128202; Benchmarks</h1>
	</div>

	<!-- Error banner -->
	{#if error}
		<div class="mb-4 rounded-md bg-accent-red/20 px-4 py-3 text-sm text-accent-red">{error}</div>
	{/if}

	<!-- Tab navigation -->
	<div class="flex gap-2 mb-6 border-b border-border-default">
		<button
			class="px-4 py-2 text-sm font-medium transition-colors border-b-2 -mb-px"
			class:border-accent-blue={activeTab === 'standard'}
			class:text-accent-blue={activeTab === 'standard'}
			class:border-transparent={activeTab !== 'standard'}
			class:text-text-muted={activeTab !== 'standard'}
			onclick={() => (activeTab = 'standard')}
		>
			Standard
		</button>
		<button
			class="px-4 py-2 text-sm font-medium transition-colors border-b-2 -mb-px"
			class:border-accent-blue={activeTab === 'mtp'}
			class:text-accent-blue={activeTab === 'mtp'}
			class:border-transparent={activeTab !== 'mtp'}
			class:text-text-muted={activeTab !== 'mtp'}
			onclick={() => (activeTab = 'mtp')}
		>
			MTP
		</button>
		<button
			class="px-4 py-2 text-sm font-medium transition-colors border-b-2 -mb-px"
			class:border-accent-blue={activeTab === 'spec'}
			class:text-accent-blue={activeTab === 'spec'}
			class:border-transparent={activeTab !== 'spec'}
			class:text-text-muted={activeTab !== 'spec'}
			onclick={() => (activeTab = 'spec')}
		>
			Spec Decoding
		</button>
		<button
			class="px-4 py-2 text-sm font-medium transition-colors border-b-2 -mb-px"
			class:border-accent-blue={activeTab === 'history'}
			class:text-accent-blue={activeTab === 'history'}
			class:border-transparent={activeTab !== 'history'}
			class:text-text-muted={activeTab !== 'history'}
			onclick={() => (activeTab = 'history')}
		>
			History ({history.length})
		</button>
	</div>

	<!-- Job log panel (shown across all tabs when running) -->
	{#if activeJobId}
		<div class="mb-6">
			<JobLogPanel
				jobId={activeJobId}
				title={jobTitle}
				onClose={() => (activeJobId = null)}
				onResult={handleBenchmarkResult}
				onDone={handleBenchmarkDone}
			/>
		</div>
	{/if}

	<!-- Benchmark result display -->
	{#if benchmarkResultParsed}
		<div class="mb-6 card">
			<h3 class="font-medium text-text-primary mb-2">Results</h3>
			<p class="text-sm text-text-muted mb-2">{getResultSummary()}</p>
			{#if benchmarkResultParsed.summaries}
				<div class="overflow-x-auto">
					<table class="w-full text-sm">
						<thead>
							<tr class="text-text-muted border-b border-border-default">
								<th class="text-left py-2 px-3">PP</th>
								<th class="text-left py-2 px-3">TG</th>
								<th class="text-right py-2 px-3">TG Mean (t/s)</th>
								<th class="text-right py-2 px-3">TG Stddev</th>
								<th class="text-right py-2 px-3">PP Mean (t/s)</th>
								<th class="text-left py-2 px-3">Status</th>
							</tr>
						</thead>
						<tbody>
							{#each benchmarkResultParsed.summaries as s}
								<tr class="border-b border-border-default/50">
									<td class="py-1.5 px-3">{s.prompt_tokens ?? '—'}</td>
									<td class="py-1.5 px-3">{s.gen_tokens ?? '—'}</td>
									<td class="py-1.5 px-3 text-right">{s.tg_mean != null ? Number(s.tg_mean).toFixed(2) : '—'}</td>
									<td class="py-1.5 px-3 text-right">{s.tg_stddev != null ? Number(s.tg_stddev).toFixed(2) : '—'}</td>
									<td class="py-1.5 px-3 text-right">{s.pp_mean != null ? Number(s.pp_mean).toFixed(2) : '—'}</td>
									<td class="py-1.5 px-3">{s.status ?? '—'}</td>
								</tr>
							{/each}
						</tbody>
					</table>
				</div>
			{/if}
		</div>
	{/if}

	<!-- ── Standard Tab ──────────────────────────────────────────────── -->
	{#if activeTab === 'standard'}
		<div class="card">
			<h2 class="text-lg font-semibold text-text-primary mb-4">Standard Benchmark (llama-bench)</h2>

			<div class="grid grid-cols-1 md:grid-cols-2 gap-4 mb-4">
				<!-- Model select -->
				<div>
					<label class="mb-1 block text-sm font-medium text-text-primary">Model</label>
					<select
						class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
						bind:value={stdModelId}
					>
						<option value="">-- Select model --</option>
						{#each modelOptions as m}
							<option value={m.id}>{m.label}</option>
						{/each}
					</select>
				</div>

				<!-- Preset -->
				<div>
					<label class="mb-1 block text-sm font-medium text-text-primary">Preset</label>
					<select
						class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
						bind:value={stdPresetIdx}
					>
						{#each BENCHMARK_PRESETS as preset, i}
							<option value={i}>{preset.label}</option>
						{/each}
					</select>
					{#if BENCHMARK_PRESETS[stdPresetIdx]}
						<p class="mt-1 text-xs text-text-muted">{BENCHMARK_PRESETS[stdPresetIdx].description}</p>
					{/if}
				</div>
			</div>

			<!-- Preset apply button -->
			<div class="mb-4">
				<button
					class="btn btn-secondary btn-sm"
					onclick={() => applyPreset(BENCHMARK_PRESETS[stdPresetIdx])}
				>
					Apply Preset
				</button>
			</div>

			<!-- Form fields -->
			<div class="grid grid-cols-2 md:grid-cols-4 gap-3 mb-4">
				<div>
					<label class="mb-1 block text-sm font-medium text-text-primary">PP Sizes</label>
					<input
						type="text"
						class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
						placeholder="2048"
						bind:value={stdPpSizes}
					/>
				</div>
				<div>
					<label class="mb-1 block text-sm font-medium text-text-primary">TG Sizes</label>
					<input
						type="text"
						class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
						placeholder="128"
						bind:value={stdTgSizes}
					/>
				</div>
				<div>
					<label class="mb-1 block text-sm font-medium text-text-primary">Runs</label>
					<input
						type="number"
						class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
						min="1"
						bind:value={stdRuns}
					/>
				</div>
				<div>
					<label class="mb-1 block text-sm font-medium text-text-primary">Threads</label>
					<input
						type="text"
						class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
						placeholder="auto"
						bind:value={stdThreads}
					/>
				</div>
			</div>

			<div class="grid grid-cols-2 md:grid-cols-4 gap-3 mb-4">
				<div>
					<label class="mb-1 block text-sm font-medium text-text-primary">NGG Range</label>
					<input
						type="text"
						class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
						placeholder="99"
						bind:value={stdNglRange}
					/>
				</div>
				<div>
					<label class="mb-1 block text-sm font-medium text-text-primary">Batch Sizes</label>
					<input
						type="text"
						class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
						placeholder="4096"
						bind:value={stdBatchSizes}
					/>
				</div>
				<div>
					<label class="mb-1 block text-sm font-medium text-text-primary">Ubatch Sizes</label>
					<input
						type="text"
						class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
						placeholder="2048"
						bind:value={stdUbatchSizes}
					/>
				</div>
				<div>
					<label class="mb-1 block text-sm font-medium text-text-primary">KV Cache Type</label>
					<input
						type="text"
						class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
						placeholder="default"
						bind:value={stdKvCacheType}
					/>
				</div>
			</div>

			<div class="grid grid-cols-2 md:grid-cols-3 gap-3 mb-4">
				<div>
					<label class="mb-1 block text-sm font-medium text-text-primary">Depth</label>
					<input
						type="text"
						class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
						placeholder="0"
						bind:value={stdDepth}
					/>
				</div>
				<div class="flex items-end">
					<div class="flex items-center gap-2 mb-2">
						<input
							id="std-flash-attn"
							type="checkbox"
							class="rounded border-border-default bg-bg-primary text-accent-blue focus:ring-accent-blue/50"
							bind:checked={stdFlashAttn}
						/>
						<label class="text-sm text-text-primary" for="std-flash-attn">Flash Attention</label>
					</div>
				</div>
			</div>

			<div class="flex justify-end">
				<button
					class="btn btn-primary"
					onclick={handleRunStandard}
					disabled={!stdModelId || running}
				>
					{running ? 'Running...' : 'Run Benchmark'}
				</button>
			</div>
		</div>
	{/if}

	<!-- ── MTP Tab ──────────────────────────────────────────────────── -->
	{#if activeTab === 'mtp'}
		<div class="card">
			<h2 class="text-lg font-semibold text-text-primary mb-4">MTP Benchmark (Multi-Token Prediction)</h2>

			<div class="grid grid-cols-1 md:grid-cols-2 gap-4 mb-4">
				<div>
					<label class="mb-1 block text-sm font-medium text-text-primary">Model</label>
					<select
						class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
						bind:value={mtpModelId}
					>
						<option value="">-- Select model --</option>
						{#each modelOptions as m}
							<option value={m.id}>{m.label}</option>
						{/each}
					</select>
				</div>
				<div>
					<label class="mb-1 block text-sm font-medium text-text-primary">Draft Max Values</label>
					<input
						type="text"
						class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
						placeholder="0,1,2,3,4,5,6,7,8"
						bind:value={mtpDraftMax}
					/>
				</div>
			</div>

			<div class="grid grid-cols-2 md:grid-cols-4 gap-3 mb-4">
				<div>
					<label class="mb-1 block text-sm font-medium text-text-primary">NGL</label>
					<input
						type="text"
						class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
						placeholder="99"
						bind:value={mtpNgl}
					/>
				</div>
				<div>
					<label class="mb-1 block text-sm font-medium text-text-primary">Draft NGL</label>
					<input
						type="text"
						class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
						placeholder="99"
						bind:value={mtpDraftNgl}
					/>
				</div>
				<div>
					<label class="mb-1 block text-sm font-medium text-text-primary">Context Size</label>
					<input
						type="text"
						class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
						placeholder="32768"
						bind:value={mtpContextSize}
					/>
				</div>
				<div class="flex items-end">
					<div class="flex items-center gap-2 mb-2">
						<input
							id="mtp-flash-attn"
							type="checkbox"
							class="rounded border-border-default bg-bg-primary text-accent-blue focus:ring-accent-blue/50"
							bind:checked={mtpFlashAttn}
						/>
						<label class="text-sm text-text-primary" for="mtp-flash-attn">Flash Attention</label>
					</div>
				</div>
			</div>

			<div class="flex justify-end">
				<button
					class="btn btn-primary"
					onclick={handleRunMtp}
					disabled={!mtpModelId || running}
				>
					{running ? 'Running...' : 'Run MTP Benchmark'}
				</button>
			</div>
		</div>
	{/if}

	<!-- ── Spec Decoding Tab ────────────────────────────────────────── -->
	{#if activeTab === 'spec'}
		<div class="card">
			<h2 class="text-lg font-semibold text-text-primary mb-4">Spec Decoding Benchmark</h2>

			<div class="grid grid-cols-1 md:grid-cols-2 gap-4 mb-4">
				<div>
					<label class="mb-1 block text-sm font-medium text-text-primary">Model</label>
					<select
						class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
						bind:value={specModelId}
					>
						<option value="">-- Select model --</option>
						{#each modelOptions as m}
							<option value={m.id}>{m.label}</option>
						{/each}
					</select>
				</div>
				<div>
					<label class="mb-1 block text-sm font-medium text-text-primary">Spec Type</label>
					<select
						class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
						bind:value={specType}
					>
						<option value="ngram">N-gram</option>
						<option value="ngram-mod">N-gram Mod</option>
						<option value="incontext">In-context</option>
						<option value="mtp">MTP</option>
					</select>
				</div>
			</div>

			<!-- Preset -->
			<div class="mb-4">
				<label class="mb-1 block text-sm font-medium text-text-primary">Preset</label>
				<div class="flex gap-2">
					{#each SPEC_PRESETS as preset, i}
						<button
							class="btn btn-sm {i === specPresetIdx ? 'btn-primary' : 'btn-secondary'}"
							onclick={() => {
								specPresetIdx = i;
								applySpecPreset(preset);
							}}
						>
							{preset.label}
						</button>
					{/each}
				</div>
			</div>

			<div class="grid grid-cols-2 md:grid-cols-4 gap-3 mb-4">
				<div>
					<label class="mb-1 block text-sm font-medium text-text-primary">Draft Max</label>
					<input
						type="text"
						class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
						placeholder="256"
						bind:value={specDraftMax}
					/>
				</div>
				<div>
					<label class="mb-1 block text-sm font-medium text-text-primary">N-gram N</label>
					<input
						type="text"
						class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
						placeholder="16"
						bind:value={specNgramN}
					/>
				</div>
				<div>
					<label class="mb-1 block text-sm font-medium text-text-primary">N-gram M</label>
					<input
						type="text"
						class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
						placeholder="12"
						bind:value={specNgramM}
					/>
				</div>
				<div>
					<label class="mb-1 block text-sm font-medium text-text-primary">N-gram Max</label>
					<input
						type="text"
						class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
						placeholder="48"
						bind:value={specNgramMax}
					/>
				</div>
			</div>

			<div class="grid grid-cols-2 md:grid-cols-4 gap-3 mb-4">
				<div>
					<label class="mb-1 block text-sm font-medium text-text-primary">N-gram Min Hits</label>
					<input
						type="number"
						class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
						min="1"
						bind:value={specNgramMinHits}
					/>
				</div>
				<div>
					<label class="mb-1 block text-sm font-medium text-text-primary">Gen Tokens</label>
					<input
						type="number"
						class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
						min="1"
						bind:value={specGenTokens}
					/>
				</div>
				<div>
					<label class="mb-1 block text-sm font-medium text-text-primary">Runs</label>
					<input
						type="number"
						class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
						min="1"
						bind:value={specRuns}
					/>
				</div>
				<div class="flex items-end">
					<div class="flex items-center gap-2 mb-2">
						<input
							id="spec-flash-attn"
							type="checkbox"
							class="rounded border-border-default bg-bg-primary text-accent-blue focus:ring-accent-blue/50"
							bind:checked={specFlashAttn}
						/>
						<label class="text-sm text-text-primary" for="spec-flash-attn">Flash Attention</label>
					</div>
				</div>
			</div>

			<div class="flex justify-end">
				<button
					class="btn btn-primary"
					onclick={handleRunSpec}
					disabled={!specModelId || running}
				>
					{running ? 'Running...' : 'Run Spec Benchmark'}
				</button>
			</div>
		</div>
	{/if}

	<!-- ── History Tab ──────────────────────────────────────────────── -->
	{#if activeTab === 'history'}
		{#if history.length === 0}
			<div class="card flex flex-col items-center justify-center py-12 gap-3">
				<p class="text-muted">No benchmark history yet</p>
			</div>
		{:else}
			<div class="card overflow-x-auto">
				<table class="w-full text-sm">
					<thead>
						<tr class="text-text-muted border-b border-border-default">
							<th class="text-left py-2 px-3">Time</th>
							<th class="text-left py-2 px-3">Model</th>
							<th class="text-left py-2 px-3">Backend</th>
							<th class="text-left py-2 px-3">Type</th>
							<th class="text-left py-2 px-3">Engine</th>
							<th class="text-right py-2 px-3">Results</th>
							<th class="text-left py-2 px-3">Status</th>
							<th class="text-right py-2 px-3">Actions</th>
						</tr>
					</thead>
					<tbody>
						{#each history as entry}
							<tr class="border-b border-border-default/50 hover:bg-bg-tertiary/50 transition-colors">
								<td class="py-2 px-3 text-text-muted">{formatBenchmarkTime(entry.created_at)}</td>
								<td class="py-2 px-3">
									<span class="font-medium">{getHistoryDisplayName(entry)}</span>
									{#if entry.quant}
										<span class="badge badge-info ml-1">{entry.quant}</span>
									{/if}
								</td>
								<td class="py-2 px-3">{entry.backend}</td>
								<td class="py-2 px-3">{entry.benchmark_type ?? '—'}</td>
								<td class="py-2 px-3">{entry.engine ?? '—'}</td>
								<td class="py-2 px-3 text-right">{entry.results_count}</td>
								<td class="py-2 px-3">
									<span class="badge {entry.status === 'success' ? 'badge-success' : 'badge-danger'}">
										{entry.status}
									</span>
								</td>
								<td class="py-2 px-3 text-right">
									<button
										class="btn btn-danger btn-sm"
										onclick={() => handleDeleteHistory(entry.id)}
									>
										&#128465;
									</button>
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		{/if}
	{/if}
</div>
