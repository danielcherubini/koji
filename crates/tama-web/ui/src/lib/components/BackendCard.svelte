<script lang="ts">
	import type { BackendCardDto, GpuTypeDto } from '$lib/types/backends';
	import { gpuTypeLabel, formatGpuVariant } from '$lib/types/backends';

	interface Props {
		backend: BackendCardDto;
		onInstall?: (type: string) => void;
		onUpdate?: (type: string, variant: string) => void;
		onCheckUpdates?: (type: string, variant: string) => void;
		onDelete?: (type: string, variant: string) => void;
		onDefaultArgsChange?: (key: string, value: string) => void;
		onVersionChange?: (type: string, version: string, variant: string) => void;
	}

	let {
		backend,
		onInstall,
		onUpdate,
		onCheckUpdates,
		onDelete,
		onDefaultArgsChange,
		onVersionChange
	}: Props = $props();

	// Track selected version index for multi-version backends
	let selectedVersionIdx = $state(
		backend.versions.findIndex((v) => v.is_active) ?? 0
	);

	// Default args text (space-separated)
	let defaultArgsText = $state(backend.default_args.join(' '));

	let selectedVersion = $derived.by(() => {
		if (selectedVersionIdx < 0 || selectedVersionIdx >= backend.versions.length) return null;
		return backend.versions[selectedVersionIdx];
	});

	let isUpdateAvailable = $derived(!!backend.update.update_available);
	let isInstalled = backend.installed;
	let versionCount = backend.versions.length;

	function handleDefaultArgsInput(e: Event) {
		const target = e.target as HTMLInputElement;
		defaultArgsText = target.value;
		if (onDefaultArgsChange) {
			const key = `${backend.type}:${backend.gpu_variant}`;
			onDefaultArgsChange(key, target.value);
		}
	}

	function handleVersionChange(e: Event) {
		const target = e.target as HTMLSelectElement;
		const idx = parseInt(target.value, 10);
		selectedVersionIdx = idx;
		if (idx >= 0 && idx < backend.versions.length && onVersionChange) {
			const v = backend.versions[idx];
			onVersionChange(backend.type, v.version, v.gpu_variant);
		}
	}

	function getStatusBadgeClass(): string {
		if (!isInstalled) return 'badge-info';
		if (backend.is_active) return 'badge-success';
		return '';
	}

	function getStatusLabel(): string {
		if (!isInstalled) return 'Not installed';
		if (backend.is_active) return 'Active';
		return 'Installed';
	}
</script>

<div class="card border-l-4" class:border-accent-green={isInstalled && backend.is_active} class:border-text-muted={!isInstalled} class:border-text-secondary={isInstalled && !backend.is_active}>
	<!-- Header: name, variant, status -->
	<div class="flex items-center gap-2 mb-2 flex-wrap">
		<span class="font-medium text-text-primary text-base">{backend.display_name}</span>

		{#if backend.gpu_variant}
			<span class="badge badge-info">{formatGpuVariant(backend.gpu_variant)}</span>
		{/if}

		<span class="badge {getStatusBadgeClass()}">{getStatusLabel()}</span>

		{#if isUpdateAvailable}
			<span class="badge badge-warning">Update available</span>
		{/if}

		{#if versionCount > 1}
			<span class="badge">{versionCount} versions</span>
		{/if}
	</div>

	<!-- Version selector -->
	{#if isInstalled && versionCount > 1}
		<div class="flex items-center gap-2 mb-2">
			<label class="text-sm font-medium text-text-secondary">Version:</label>
			<select
				class="rounded-md border border-border-default bg-bg-primary px-2 py-1 text-sm text-text-primary focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
				value={selectedVersionIdx}
				onchange={handleVersionChange}
			>
				{#each backend.versions as v, i}
					<option value={i}>
						{v.version}{v.is_active ? ' (active)' : ''}
					</option>
				{/each}
			</select>
		</div>
	{/if}

	<!-- Version info -->
	{#if selectedVersion}
		<div class="text-sm text-text-secondary mb-2 space-y-0.5">
			<div><span class="font-medium">Version:</span> {selectedVersion.version}</div>
			{#if selectedVersion.gpu_type}
				<div><span class="font-medium">GPU:</span> {gpuTypeLabel(selectedVersion.gpu_type)}</div>
			{/if}
			<div class="text-text-muted"><span class="font-medium">Path:</span> <code>{selectedVersion.path}</code></div>
		</div>
	{/if}

	{#if isUpdateAvailable && backend.update.latest_version}
		<div class="text-sm text-accent-blue mb-2">
			<span class="font-medium">Latest:</span> {backend.update.latest_version}
		</div>
	{/if}

	<!-- Default args editor -->
	<div class="mb-3">
		<label class="text-sm font-medium text-text-secondary mb-1 block">Default Args</label>
		<input
			type="text"
			class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-1.5 text-sm text-text-primary placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-blue/50 font-mono"
			placeholder="No default args set"
			value={defaultArgsText}
			oninput={handleDefaultArgsInput}
		/>
	</div>

	<!-- Actions -->
	<div class="flex items-center gap-2 flex-wrap">
		{#if !isInstalled}
			<button
				class="btn btn-primary btn-sm"
				onclick={() => onInstall?.(backend.type)}
			>
				Install
			</button>
		{:else}
			{#if onCheckUpdates}
				<button
					class="btn btn-secondary btn-sm"
					onclick={() => onCheckUpdates?.(backend.type, backend.gpu_variant)}
				>
					Check for updates
				</button>
			{/if}

			{#if isUpdateAvailable && onUpdate}
				<button
					class="btn btn-primary btn-sm"
					onclick={() => onUpdate?.(backend.type, backend.gpu_variant)}
				>
					Update
				</button>
			{/if}

			{#if backend.is_active && onDelete}
				<button
					class="btn btn-danger btn-sm"
					onclick={() => onDelete?.(backend.type, backend.gpu_variant)}
				>
					Uninstall
				</button>
			{/if}
		{/if}

		{#if backend.release_notes_url}
			<a
				href={backend.release_notes_url}
				target="_blank"
				rel="noopener noreferrer"
				class="btn btn-secondary btn-sm"
			>
				Release notes
			</a>
		{/if}
	</div>
</div>
