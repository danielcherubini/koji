<script lang="ts">
	import { fetchLogs } from '$lib/api/logs';
	import type { SourceLogs } from '$lib/types/logs';
	import { onMount } from 'svelte';

	let sources = $state<SourceLogs[]>([]);
	let selectedSource = $state('tama');
	let loading = $state(true);
	let error = $state<string | null>(null);
	let interval: ReturnType<typeof setInterval> | null = null;

	async function loadLogs() {
		loading = true;
		error = null;
		try {
			const data = await fetchLogs();
			sources = data.sources;
		} catch (e: any) {
			error = e.message || 'Failed to load logs.';
			sources = [];
		}
		loading = false;
	}

	onMount(() => {
		loadLogs();
		interval = setInterval(loadLogs, 5000);
		return () => {
			if (interval) clearInterval(interval);
		};
	});

	function handleRefresh() {
		loadLogs();
	}

	function logLevelClass(line: string): string {
		const upper = line.toUpperCase();
		if (upper.includes('ERROR') || upper.includes('FATAL')) return 'log-error';
		if (upper.includes('WARN')) return 'log-warn';
		if (upper.includes('DEBUG')) return 'log-debug';
		return 'log-info';
	}

	let activeSourceLogs = $derived(
		sources
			.filter((s) => !selectedSource || s.name === selectedSource)
			.flatMap((s) => {
				const header = selectedSource === '' ? [`=== ${s.name} ===`] : [];
				const lines = header.concat(s.lines);
				return lines.map((line) => ({ line, isHeader: line.startsWith('===') }));
			})
	);
</script>

<div class="page">
	<div class="page-header">
		<h1>&#128203; Log Viewer</h1>
		<div class="page-header-actions">
			<select
				class="rounded-md border border-border-default bg-bg-primary px-3 py-1.5 text-sm text-text-primary focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
				bind:value={selectedSource}
			>
				<option value="tama">tama</option>
				{#each sources.filter(s => s.name !== 'tama') as s (s.name)}
					<option value={s.name}>{s.name}</option>
				{/each}
			</select>
			<button class="btn btn-secondary" onclick={handleRefresh} disabled={loading}>
				&#8633; Refresh
			</button>
		</div>
	</div>

	{#if loading && sources.length === 0}
		<div class="card flex items-center justify-center gap-3 py-12">
			<div class="h-5 w-5 animate-spin rounded-full border-2 border-text-muted border-t-accent-blue"></div>
			<span class="text-muted">Loading logs...</span>
		</div>
	{:else if error}
		<div class="rounded-md bg-accent-yellow/20 px-4 py-3 text-sm">
			<span class="text-accent-yellow font-medium">&#9888;</span>
			<span class="ml-2 text-text-secondary">{error}</span>
		</div>
	{:else if activeSourceLogs.length === 0}
		<div class="card flex items-center justify-center py-12">
			<span class="text-muted">No logs yet...</span>
		</div>
	{:else}
		<div class="card overflow-hidden p-0">
			<pre class="log-viewer">
				{#each activeSourceLogs as entry}
					<div
						class="log-line"
						class:log-header={entry.isHeader}
						class:log-error={!entry.isHeader && logLevelClass(entry.line) === 'log-error'}
						class:log-warn={!entry.isHeader && logLevelClass(entry.line) === 'log-warn'}
						class:log-debug={!entry.isHeader && logLevelClass(entry.line) === 'log-debug'}
						class:log-info={!entry.isHeader && logLevelClass(entry.line) === 'log-info'}
					>
						<span class="log-line-text">{entry.line}</span>
					</div>
				{/each}
			</pre>
		</div>
	{/if}
</div>

<style>
	.log-viewer {
		font-family: var(--font-mono);
		font-size: 0.8125rem;
		line-height: 1.6;
		background: #0d1117;
		color: var(--color-text-primary);
		overflow-x: auto;
		overflow-y: auto;
		max-height: 70vh;
		padding: 0.75rem;
		white-space: pre-wrap;
		word-break: break-all;
		margin: 0;
	}

	.log-line {
		padding: 0.0625rem 0;
	}

	.log-header {
		color: var(--color-accent-cyan);
		font-weight: 700;
	}

	.log-error {
		color: var(--color-accent-red);
	}

	.log-warn {
		color: var(--color-accent-yellow);
	}

	.log-debug {
		color: var(--color-text-muted);
		opacity: 0.7;
	}

	.log-info {
		color: var(--color-text-secondary);
	}
</style>
