<script lang="ts">
	import Modal from '$lib/components/Modal.svelte';
	import type { InstallRequest, CapabilitiesDto, GpuTypeDto } from '$lib/types/backends';
	import { gpuTypeLabel } from '$lib/types/backends';

	interface Props {
		open: boolean;
		backendType: string;
		capabilities: CapabilitiesDto;
		onSubmit: (request: InstallRequest) => void;
		onClose: () => void;
	}

	let { open, backendType, capabilities, onSubmit, onClose }: Props = $props();

	// Form state
	let gpuKind = $state('cpu');
	let cudaVersion = $state('12.4');
	let version = $state('latest');
	let buildFromSource = $state(false);
	let forceOverwrite = $state(false);
	let error = $state<string | null>(null);

	// Derived
	let isIkLlama = $derived(backendType === 'ik_llama');
	let isLinux = $derived(capabilities.os === 'linux');
	let canBuild = $derived(
		capabilities.git_available &&
		capabilities.cmake_available &&
		capabilities.compiler_available
	);

	// Force source build for ik_llama or linux+cuda
	let forceSource = $derived(isIkLlama || (isLinux && gpuKind === 'cuda'));
	let effectiveBuildFromSource = $derived(forceSource || buildFromSource);

	let displayName = $derived(
		backendType === 'llama_cpp'
			? 'llama.cpp'
			: backendType === 'ik_llama'
				? 'ik_llama.cpp'
				: backendType === 'tts_kokoro'
					? 'Kokoro TTS'
					: backendType
	);

	// Reset form when modal opens with a new backend type
	$effect(() => {
		if (open) {
			error = null;
			gpuKind = capabilities.detected_cuda_version ? 'cuda' : 'cpu';
			cudaVersion =
				capabilities.detected_cuda_version ||
				capabilities.supported_cuda_versions[0] ||
				'12.4';
			version = isIkLlama ? '' : 'latest';
			buildFromSource = false;
			forceOverwrite = false;
		}
	});

	function handleSubmit() {
		error = null;

		const gpuType: GpuTypeDto =
			gpuKind === 'cuda'
				? { kind: 'cuda', version: cudaVersion }
				: gpuKind === 'vulkan'
					? { kind: 'vulkan' }
					: gpuKind === 'metal'
						? { kind: 'metal' }
						: gpuKind === 'rocm'
							? { kind: 'rocm', version: '7.2' }
							: { kind: 'cpu_only' };

		const request: InstallRequest = {
			backend_type: backendType,
			version: isIkLlama ? undefined : version === 'latest' ? undefined : version,
			gpu_type: gpuType,
			build_from_source: effectiveBuildFromSource,
			force: forceOverwrite
		};

		onSubmit(request);
	}

	function getGpuLabel(): string {
		const kind =
			gpuKind === 'cuda'
				? { kind: 'cuda' as const, version: cudaVersion }
				: gpuKind === 'vulkan'
					? { kind: 'vulkan' as const }
					: gpuKind === 'metal'
						? { kind: 'metal' as const }
						: gpuKind === 'rocm'
							? { kind: 'rocm' as const, version: '7.2' }
							: { kind: 'cpu_only' as const };
		return gpuTypeLabel(kind);
	}
</script>

<Modal open={open} onClose={onClose} title={`Install ${displayName}`}>
	<div class="space-y-4">
		<!-- Prerequisites warning -->
		{#if effectiveBuildFromSource && !canBuild}
			<div class="rounded-md bg-accent-yellow/20 px-3 py-2 text-sm text-accent-yellow">
				&#9888;&#65039; Build prerequisites missing (git/cmake/compiler). Source builds may fail.
			</div>
		{/if}

		<!-- GPU Acceleration -->
		<div>
			<label class="mb-1 block text-sm font-medium text-text-primary" for="gpu-kind">
				GPU Acceleration
			</label>
			<select
				id="gpu-kind"
				class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
				bind:value={gpuKind}
			>
				<option value="cpu">CPU Only</option>
				<option value="cuda">CUDA (NVIDIA)</option>
				<option value="vulkan">Vulkan</option>
				<option value="metal">Metal (macOS)</option>
				<option value="rocm">ROCm (AMD)</option>
			</select>
			{#if capabilities.detected_cuda_version}
				<p class="mt-1 text-xs text-text-muted">
					Detected CUDA {capabilities.detected_cuda_version}
				</p>
			{/if}
		</div>

		<!-- CUDA version -->
		{#if gpuKind === 'cuda'}
			<div>
				<label class="mb-1 block text-sm font-medium text-text-primary" for="cuda-version">
					CUDA Version
				</label>
				<select
					id="cuda-version"
					class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
					bind:value={cudaVersion}
				>
					{#each capabilities.supported_cuda_versions as v}
						<option value={v}>{v}</option>
					{/each}
				</select>
			</div>
		{/if}

		<!-- Version -->
		{#if !isIkLlama}
			<div>
				<label class="mb-1 block text-sm font-medium text-text-primary" for="version">
					Version
				</label>
				<input
					id="version"
					type="text"
					class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
					placeholder="latest"
					bind:value={version}
				/>
				<p class="mt-1 text-xs text-text-muted">
					Use 'latest' or a specific tag like 'b8407'.
				</p>
			</div>
		{:else}
			<div class="rounded-md bg-accent-blue/20 px-3 py-2 text-sm text-accent-blue">
				ik_llama is built from the latest main branch commit.
			</div>
		{/if}

		<!-- Build from source -->
		<div class="flex items-center gap-2">
			<input
				id="build-source"
				type="checkbox"
				class="rounded border-border-default bg-bg-primary text-accent-blue focus:ring-accent-blue/50"
				bind:checked={buildFromSource}
				disabled={forceSource}
			/>
			<label class="text-sm text-text-primary" for="build-source">
				Build from source
			</label>
		</div>
		{#if forceSource}
			<p class="text-xs text-text-muted ml-6">
				Forced: {isIkLlama ? 'ik_llama always builds from source' : 'No prebuilt CUDA binary for Linux — source build required'}
			</p>
		{/if}

		<!-- Force overwrite -->
		<div class="flex items-center gap-2">
			<input
				id="force"
				type="checkbox"
				class="rounded border-border-default bg-bg-primary text-accent-blue focus:ring-accent-blue/50"
				bind:checked={forceOverwrite}
			/>
			<label class="text-sm text-text-primary" for="force">
				Force overwrite existing installation
			</label>
		</div>

		<!-- Summary -->
		<div class="rounded-md bg-bg-tertiary p-3 text-xs text-text-secondary space-y-1">
			<p><span class="font-medium">Backend:</span> {displayName}</p>
			<p><span class="font-medium">GPU:</span> {getGpuLabel()}</p>
			{#if !isIkLlama}
				<p><span class="font-medium">Version:</span> {version || 'latest'}</p>
			{/if}
			<p><span class="font-medium">Build from source:</span> {effectiveBuildFromSource ? 'Yes' : 'No'}</p>
		</div>

		<!-- Error -->
		{#if error}
			<div class="rounded-md bg-accent-red/20 px-3 py-2 text-sm text-accent-red">{error}</div>
		{/if}

		<!-- Actions -->
		<div class="flex justify-end gap-2">
			<button type="button" class="btn btn-secondary" onclick={onClose}>Cancel</button>
			<button type="button" class="btn btn-primary" onclick={handleSubmit}>Install</button>
		</div>
	</div>
</Modal>
