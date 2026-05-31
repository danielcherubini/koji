<script lang="ts">
	import { formatSize } from '$lib/utils/formatting';

	interface DownloadProgress {
		filename: string;
		bytes_downloaded: number;
		total_bytes: number | null;
		status: string;
		error?: string;
	}

	interface Props {
		progress: DownloadProgress[];
		onCancel: () => void;
	}

	let { progress, onCancel }: Props = $props();

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
			case 'cancelled':
				return 'badge-danger';
			default:
				return 'badge-info';
		}
	}

	function getProgressPct(item: DownloadProgress): number {
		if (!item.total_bytes || item.total_bytes === 0) return 0;
		return Math.min((item.bytes_downloaded / item.total_bytes) * 100, 100);
	}
</script>

<div class="space-y-4">
	<h4 class="text-sm font-medium text-text-primary">Downloading files...</h4>

	<div class="space-y-3 max-h-64 overflow-y-auto">
		{#each progress as item (item.filename)}
			<div class="space-y-1">
				<div class="flex items-center justify-between">
					<div class="flex items-center gap-2 min-w-0">
						<span class="text-sm text-text-primary truncate">{item.filename}</span>
						<span class="badge {getStatusBadge(item.status)} shrink-0">{getStatusLabel(item.status)}</span>
					</div>
				</div>

				<!-- Progress bar -->
				<div class="h-1.5 w-full rounded-full bg-bg-tertiary overflow-hidden">
					<div
						class="h-full rounded-full bg-accent-blue transition-all duration-300"
						style="width: {getProgressPct(item)}%"
					></div>
				</div>

				<div class="flex items-center justify-between text-xs text-text-muted">
					<span>{formatSize(item.bytes_downloaded)}{item.total_bytes ? ` / ${formatSize(item.total_bytes)}` : ''}</span>
				</div>

				{#if item.error}
					<div class="text-xs text-accent-red">{item.error}</div>
				{/if}
			</div>
		{/each}
	</div>

	<div class="flex justify-end pt-2">
		<button class="btn btn-secondary" onclick={onCancel}>Cancel</button>
	</div>
</div>
