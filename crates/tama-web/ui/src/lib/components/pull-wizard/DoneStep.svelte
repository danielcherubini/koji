<script lang="ts">
	import { formatSize } from '$lib/utils/formatting';

	interface CompletedQuant {
		filename: string;
		quant?: string;
		size_bytes?: number;
	}

	interface Props {
		completed: CompletedQuant[];
		onClose: () => void;
	}

	let { completed, onClose }: Props = $props();
</script>

<div class="space-y-4 text-center">
	<div class="text-4xl">&#10004;</div>
	<h4 class="text-lg font-semibold text-accent-green">Pull Complete</h4>
	<p class="text-sm text-text-secondary">
		{completed.length} file(s) pulled successfully.
	</p>

	{#if completed.length > 0}
		<div class="text-left max-h-40 overflow-y-auto space-y-1">
			{#each completed as item (item.filename)}
				<div class="flex items-center justify-between text-sm py-1">
					<span class="text-text-primary truncate">{item.filename}</span>
					<span class="text-text-muted shrink-0 ml-2">
						{item.size_bytes ? formatSize(item.size_bytes) : ''}
						{item.quant ? ` (${item.quant})` : ''}
					</span>
				</div>
			{/each}
		</div>
	{/if}

	<div class="flex justify-center pt-2">
		<button class="btn btn-primary" onclick={onClose}>
			Close
		</button>
	</div>
</div>
