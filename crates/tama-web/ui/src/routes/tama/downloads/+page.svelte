<script lang="ts">
	import { onMount } from 'svelte';
	import { cancelDownload } from '$lib/api/downloads';
	import { formatSize } from '$lib/utils/formatting';
	import { activeDownloads as activeDownloadsStore } from '$lib/stores/downloads';
	import { addToast } from '$lib/stores/toasts';
	import type { DownloadItem } from '$lib/types/downloads';

	let { data } = $props();

	// Tab state
	let activeTab = $state('active');

	// Active downloads: seeded from page data, then updated by SSE store
	let activeDownloads = $state<DownloadItem[]>(data.activeDownloads ?? []);

	// History: seeded from page data, with pagination
	let historyItems = $state<DownloadItem[]>(data.historyItems ?? []);
	let historyTotal = $state<number>(data.historyTotal ?? 0);
	let historyPage = $state(1);
	const PAGE_SIZE = 50;

	// Subscribe to SSE store for real-time active downloads
	let unsubscribe: (() => void) | null = null;

	onMount(() => {
		unsubscribe = activeDownloadsStore.subscribe((downloads) => {
			// Map ActiveDownload from store to DownloadItem shape
			activeDownloads = downloads.map((d) => ({
				job_id: d.job_id,
				repo_id: d.filename,
				filename: d.filename,
				display_name: null,
				status: d.status,
				bytes_downloaded: d.bytes_downloaded,
				total_bytes: d.total_bytes ?? null,
				error_message: null,
				started_at: null,
				completed_at: null,
				queued_at: '',
				kind: 'file',
			}));
		});
	});

	async function handleCancel(jobId: string) {
		try {
			await cancelDownload(jobId);
			addToast('Cancelled', 'Download cancelled.', 'info');
		} catch (e: any) {
			addToast('Error', `Failed to cancel: ${e.message}`, 'error');
		}
	}

	function getStatusLabel(status: string): string {
		switch (status) {
			case 'running': return 'Downloading';
			case 'verifying': return 'Verifying';
			case 'queued': return 'Queued';
			case 'completed': return 'Completed';
			case 'failed': return 'Failed';
			case 'cancelled': return 'Cancelled';
			default: return status;
		}
	}

	function getStatusBadge(status: string): string {
		switch (status) {
			case 'running':
			case 'completed':
				return 'badge-success';
			case 'verifying':
			case 'queued':
				return 'badge-warning';
			case 'failed':
				return 'badge-danger';
			case 'cancelled':
				return 'badge-info';
			default:
				return 'badge-info';
		}
	}

	function getProgress(item: DownloadItem): number {
		if (!item.total_bytes || item.total_bytes === 0) return 0;
		return Math.min((item.bytes_downloaded / item.total_bytes) * 100, 100);
	}

	function getDisplayName(item: DownloadItem): string {
		return item.display_name || item.filename || item.repo_id;
	}

	function formatTime(isoString: string | null): string {
		if (!isoString) return '';
		try {
			return new Date(isoString).toLocaleTimeString();
		} catch {
			return '';
		}
	}

	let paginatedHistory = $derived(historyItems.slice((historyPage - 1) * PAGE_SIZE, historyPage * PAGE_SIZE));
	let totalPages = $derived(Math.max(1, Math.ceil(historyTotal / PAGE_SIZE)));

	function prevPage() {
		if (historyPage > 1) historyPage--;
	}

	function nextPage() {
		if (historyPage < totalPages) historyPage++;
	}
</script>

<div class="page">
	<div class="page-header">
		<h1>&#128230;&#65039; Downloads</h1>
	</div>

	<!-- Tab navigation -->
	<div class="flex gap-2 mb-6 border-b border-border-default">
		<button
			class="px-4 py-2 text-sm font-medium transition-colors border-b-2 -mb-px"
			class:border-accent-blue={activeTab === 'active'}
			class:text-accent-blue={activeTab === 'active'}
			class:border-transparent={activeTab !== 'active'}
			class:text-text-muted={activeTab !== 'active'}
			class:hover:text-text-primary={activeTab !== 'active'}
			onclick={() => activeTab = 'active'}
		>
			Active ({activeDownloads.length})
		</button>
		<button
			class="px-4 py-2 text-sm font-medium transition-colors border-b-2 -mb-px"
			class:border-accent-blue={activeTab === 'history'}
			class:text-accent-blue={activeTab === 'history'}
			class:border-transparent={activeTab !== 'history'}
			class:text-text-muted={activeTab !== 'history'}
			class:hover:text-text-primary={activeTab !== 'history'}
			onclick={() => activeTab = 'history'}
		>
			History ({historyTotal})
		</button>
	</div>

	<!-- Active Downloads Tab -->
	{#if activeTab === 'active'}
		{#if activeDownloads.length === 0}
			<div class="card flex flex-col items-center justify-center py-12 gap-3">
				<p class="text-muted">No active downloads</p>
			</div>
		{:else}
			<div class="flex flex-col gap-3">
				{#each activeDownloads as item (item.job_id)}
					<div class="card">
						<div class="flex items-center justify-between mb-2">
							<div class="flex items-center gap-3 min-w-0">
								<span class="font-medium text-text-primary truncate">{getDisplayName(item)}</span>
								<span class="badge {getStatusBadge(item.status)}">{getStatusLabel(item.status)}</span>
							</div>
							<div class="flex items-center gap-2 shrink-0">
								{#if item.status === 'running' || item.status === 'verifying' || item.status === 'queued'}
									<button
										class="btn btn-danger btn-sm"
										onclick={() => handleCancel(item.job_id)}
									>
										Cancel
									</button>
								{/if}
							</div>
						</div>

						<!-- Progress bar -->
						<div class="mb-2">
							<div class="h-2 w-full rounded-full bg-bg-tertiary overflow-hidden">
								<div
									class="h-full rounded-full bg-accent-blue transition-all duration-300"
									style="width: {getProgress(item)}%"
								></div>
							</div>
						</div>

						<div class="flex items-center justify-between text-xs text-text-muted">
							<span>
								{formatSize(item.bytes_downloaded)}
								{#if item.total_bytes}
									/ {formatSize(item.total_bytes)}
								{/if}
							</span>
							<span>
								{#if item.started_at}
									Started: {formatTime(item.started_at)}
								{/if}
							</span>
						</div>

						{#if item.error_message}
							<div class="mt-2 text-xs text-accent-red">{item.error_message}</div>
						{/if}
					</div>
				{/each}
			</div>
		{/if}
	{/if}

	<!-- History Tab -->
	{#if activeTab === 'history'}
		{#if historyItems.length === 0}
			<div class="card flex flex-col items-center justify-center py-12 gap-3">
				<p class="text-muted">No download history</p>
			</div>
		{:else}
			<div class="flex flex-col gap-3 mb-4">
				{#each paginatedHistory as item (item.job_id)}
					<div class="card">
						<div class="flex items-center justify-between">
							<div class="flex items-center gap-3 min-w-0">
								<span class="font-medium text-text-primary truncate">{getDisplayName(item)}</span>
								<span class="badge {getStatusBadge(item.status)}">{getStatusLabel(item.status)}</span>
							</div>
							<div class="flex items-center gap-3 shrink-0 text-xs text-text-muted">
								<span>
									{formatSize(item.bytes_downloaded)}
									{#if item.total_bytes}
										/ {formatSize(item.total_bytes)}
									{/if}
								</span>
								{#if item.completed_at}
									<span>{formatTime(item.completed_at)}</span>
								{/if}
							</div>
						</div>

						{#if item.error_message}
							<div class="mt-1 text-xs text-accent-red">{item.error_message}</div>
						{/if}
					</div>
				{/each}
			</div>

			<!-- Pagination controls -->
			{#if totalPages > 1}
				<div class="flex items-center justify-between text-sm text-text-secondary">
					<button
						class="btn btn-secondary btn-sm"
						onclick={prevPage}
						disabled={historyPage <= 1}
					>
						&#8592; Prev
					</button>
					<span>Page {historyPage} of {totalPages}</span>
					<button
						class="btn btn-secondary btn-sm"
						onclick={nextPage}
						disabled={historyPage >= totalPages}
					>
						Next &#8594;
					</button>
				</div>
			{/if}
		{/if}
	{/if}
</div>
