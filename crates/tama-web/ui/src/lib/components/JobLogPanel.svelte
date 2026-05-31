<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import { tick } from 'svelte';

	interface Props {
		jobId: string;
		title?: string;
		onClose?: () => void;
		onResult?: (results: string) => void;
		onStatus?: (status: string) => void;
		onDone?: () => void;
	}

	let { jobId, title = 'Job Log', onClose, onResult, onStatus, onDone }: Props = $props();

	let logLines = $state<string[]>([]);
	let status = $state<string>('running');
	let finished = $state(false);
	let autoScroll = $state(true);
	let scrollContainer: HTMLPreElement | null = null;

	let eventSource: EventSource | null = null;

	function connectSSE() {
		if (finished) return;
		const url = `/tama/v1/jobs/${jobId}/events`;
		eventSource = new EventSource(url);

		eventSource.addEventListener('log', (e: MessageEvent) => {
			try {
				const data = JSON.parse(e.data);
				logLines.push(data.line);
				if (logLines.length > 2000) {
					logLines = logLines.slice(-1500);
				}
				// Auto-scroll
				tick().then(() => {
					if (autoScroll && scrollContainer) {
						scrollContainer.scrollTop = scrollContainer.scrollHeight;
					}
				});
			} catch {
				logLines.push(e.data);
			}
		});

		eventSource.addEventListener('status', (e: MessageEvent) => {
			try {
				const data = JSON.parse(e.data);
				status = data.status;
				onStatus?.(data.status);
				if (data.status !== 'JobStatus::Running') {
					finished = true;
					onDone?.();
					eventSource?.close();
					eventSource = null;
				}
			} catch {
				// ignore
			}
		});

		eventSource.addEventListener('result', (e: MessageEvent) => {
			try {
				const data = JSON.parse(e.data);
				onResult?.(data.results);
			} catch {
				// ignore
			}
		});

		eventSource.onerror = () => {
			if (!finished) {
				eventSource?.close();
				eventSource = null;
				// Reconnect after a brief delay
				setTimeout(connectSSE, 2000);
			}
		};
	}

	function handleScroll() {
		if (!scrollContainer) return;
		const { scrollTop, scrollHeight, clientHeight } = scrollContainer;
		autoScroll = scrollHeight - scrollTop - clientHeight < 50;
	}

	function copyLogs() {
		const text = logLines.join('\n');
		navigator.clipboard?.writeText(text).catch(() => {
			// Fallback: ignore
		});
	}

	function clearLogs() {
		logLines = [];
	}

	onMount(() => {
		connectSSE();
	});

	onDestroy(() => {
		eventSource?.close();
		eventSource = null;
	});

	// Get status badge class
	function getStatusBadge(): string {
		if (status.includes('Running')) return 'badge-info';
		if (status.includes('Succeeded')) return 'badge-success';
		if (status.includes('Failed')) return 'badge-danger';
		if (status.includes('Cancelled')) return 'badge-warning';
		return 'badge-info';
	}

	function getStatusLabel(): string {
		if (status.includes('Running')) return 'Running';
		if (status.includes('Succeeded')) return 'Completed';
		if (status.includes('Failed')) return 'Failed';
		if (status.includes('Cancelled')) return 'Cancelled';
		return status;
	}
</script>

<div class="card" style="position: relative;">
	<!-- Header -->
	<div class="flex items-center justify-between mb-3">
		<div class="flex items-center gap-2">
			<span class="font-medium text-text-primary">{title}</span>
			<span class="badge {getStatusBadge()}">{getStatusLabel()}</span>
			{#if logLines.length > 0}
				<span class="text-xs text-text-muted">({logLines.length} lines)</span>
			{/if}
		</div>
		<div class="flex items-center gap-1">
			<button class="btn btn-secondary btn-sm" onclick={copyLogs} title="Copy logs">
				&#128203;
			</button>
			<button class="btn btn-secondary btn-sm" onclick={clearLogs} title="Clear">
				&#128465;
			</button>
			{#if onClose}
				<button class="btn btn-secondary btn-sm" onclick={onClose} title="Close">
					&#10005;
				</button>
			{/if}
		</div>
	</div>

	<!-- Log output -->
	{#if logLines.length === 0 && !finished}
		<div class="text-center py-8 text-text-muted">
			<p>Waiting for job to start...</p>
		</div>
	{:else if finished && logLines.length === 0}
		<div class="text-center py-8 text-text-muted">
			<p>Job completed with no output.</p>
		</div>
	{:else}
		<pre
			class="job-log-output"
			bind:this={scrollContainer}
			onscroll={handleScroll}
		>
			{logLines.join('\n')}
		</pre>
	{/if}
</div>

<style>
	.job-log-output {
		background: var(--color-bg-primary);
		border: 1px solid var(--color-border-default);
		border-radius: 0.5rem;
		padding: 0.75rem;
		font-family: var(--font-mono);
		font-size: 0.8125rem;
		line-height: 1.5;
		color: var(--color-text-secondary);
		white-space: pre-wrap;
		word-break: break-word;
		max-height: 400px;
		overflow-y: auto;
		overflow-x: hidden;
		tab-size: 4;
	}

	.job-log-output::-webkit-scrollbar {
		width: 6px;
	}

	.job-log-output::-webkit-scrollbar-track {
		background: transparent;
	}

	.job-log-output::-webkit-scrollbar-thumb {
		background: var(--color-border-default);
		border-radius: 3px;
	}

	.job-log-output::-webkit-scrollbar-thumb:hover {
		background: var(--color-border-hover);
	}
</style>
