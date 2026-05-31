<script lang="ts">
	import { formatSize } from '$lib/utils/formatting';

	interface HfFileInfo {
		filename: string;
		size: number;
		kind: 'model' | 'mmproj';
		quant?: string;
	}

	interface Props {
		repoId: string;
		quants: HfFileInfo[];
		mmprojs: HfFileInfo[];
		selectedFiles: string[];
		onToggle: (filename: string) => void;
		onSelectAll: () => void;
		onDeselectAll: () => void;
		onDownload: () => void;
		onBack: () => void;
	}

	let {
		repoId,
		quants,
		mmprojs,
		selectedFiles,
		onToggle,
		onSelectAll,
		onDeselectAll,
		onDownload,
		onBack
	}: Props = $props();

	let allFiles = $derived([...quants, ...mmprojs]);
	let allSelected = $derived(allFiles.length > 0 && allFiles.every((f) => selectedFiles.includes(f.filename)));
	let totalSelectedSize = $derived(
		allFiles
			.filter((f) => selectedFiles.includes(f.filename))
			.reduce((sum, f) => sum + f.size, 0)
	);

	function getKindLabel(kind: string): string {
		return kind === 'mmproj' ? 'MMProj' : 'Model';
	}
</script>

<div class="space-y-4">
	<!-- Header -->
	<div>
		<p class="text-sm text-text-secondary">
			Select files from <strong class="text-text-primary font-mono">{repoId}</strong>
		</p>
	</div>

	<!-- Selection controls -->
	<div class="flex items-center gap-2">
		<button class="btn btn-secondary btn-sm" onclick={onSelectAll}>Select All</button>
		<button class="btn btn-secondary btn-sm" onclick={onDeselectAll}>Deselect All</button>
		<span class="text-xs text-text-muted ml-auto">
			{selectedFiles.length} selected &middot; {formatSize(totalSelectedSize)}
		</span>
	</div>

	<!-- Quants list -->
	{#if quants.length > 0}
		<div>
			<h4 class="text-sm font-medium text-text-primary mb-2">Model Quants ({quants.length})</h4>
			<div class="space-y-1 max-h-48 overflow-y-auto">
				{#each quants as file (file.filename)}
					<label class="flex items-center gap-3 p-2 rounded-md hover:bg-bg-tertiary/50 cursor-pointer">
						<input
							type="checkbox"
							class="rounded border-border-default bg-bg-primary text-accent-blue focus:ring-accent-blue/50"
							checked={selectedFiles.includes(file.filename)}
							onchange={() => onToggle(file.filename)}
						/>
						<div class="flex-1 min-w-0">
							<div class="text-sm text-text-primary truncate">{file.filename}</div>
							<div class="text-xs text-text-muted">
								{file.quant ? `Quant: ${file.quant} &middot; ` : ''}{formatSize(file.size)}
							</div>
						</div>
					</label>
				{/each}
			</div>
		</div>
	{/if}

	<!-- MMProjs list -->
	{#if mmprojs.length > 0}
		<div>
			<h4 class="text-sm font-medium text-text-primary mb-2">MMProjs ({mmprojs.length})</h4>
			<div class="space-y-1 max-h-32 overflow-y-auto">
				{#each mmprojs as file (file.filename)}
					<label class="flex items-center gap-3 p-2 rounded-md hover:bg-bg-tertiary/50 cursor-pointer">
						<input
							type="checkbox"
							class="rounded border-border-default bg-bg-primary text-accent-blue focus:ring-accent-blue/50"
							checked={selectedFiles.includes(file.filename)}
							onchange={() => onToggle(file.filename)}
						/>
						<div class="flex-1 min-w-0">
							<div class="text-sm text-text-primary truncate">{file.filename}</div>
							<div class="text-xs text-text-muted">{formatSize(file.size)}</div>
						</div>
					</label>
				{/each}
			</div>
		</div>
	{/if}

	{#if allFiles.length === 0}
		<div class="text-sm text-text-muted text-center py-4">No files found in this repo.</div>
	{/if}

	<!-- Actions -->
	<div class="flex justify-between pt-2">
		<button class="btn btn-secondary" onclick={onBack}>Back</button>
		<button
			class="btn btn-primary"
			onclick={onDownload}
			disabled={selectedFiles.length === 0}
		>
			Download ({selectedFiles.length})
		</button>
	</div>
</div>
