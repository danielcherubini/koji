<script lang="ts">
	import { onMount } from 'svelte';
	import {
		listBackends,
		installBackend,
		updateBackend,
		removeBackend,
		checkAllUpdates,
		updateDefaultArgs
	} from '$lib/api/backends';
	import type { BackendCardDto, CapabilitiesDto, InstallRequest } from '$lib/types/backends';
	import BackendCard from '$lib/components/BackendCard.svelte';
	import InstallModal from '$lib/components/InstallModal.svelte';
	import JobLogPanel from '$lib/components/JobLogPanel.svelte';
	import { addToast } from '$lib/stores/toasts';

	let { data } = $props();

	let allBackends = $state<BackendCardDto[]>([
		...(data.backends ?? []),
		...(data.custom ?? [])
	]);
	let capabilities = $state<CapabilitiesDto>(data.capabilities ?? {
		os: 'unknown',
		arch: 'unknown',
		git_available: false,
		cmake_available: false,
		compiler_available: false,
		supported_cuda_versions: []
	});
	let availableBackends = $state<string[]>(data.available ?? []);

	// UI state
	let error = $state<string | null>(null);
	let saving = $state(false);
	let checking = $state(false);

	// Install modal
	let showInstallModal = $state(false);
	let installBackendType = $state('llama_cpp');

	// Job log
	let activeJobId = $state<string | null>(data.activeJob?.id ?? null);
	let jobTitle = $state('Job Progress');

	// Pending default args changes
	let pendingArgs = $state<Map<string, string>>(new Map());

	// Dropdown for "+ Add Backend"
	let showAddDropdown = $state(false);

	const knownBackends = [
		{ type: 'llama_cpp', label: 'llama.cpp' },
		{ type: 'ik_llama', label: 'ik_llama.cpp' },
		{ type: 'tts_kokoro', label: 'Kokoro TTS' }
	];

	onMount(() => {
		if (data.activeJob?.id) {
			activeJobId = data.activeJob.id;
			jobTitle = `${data.activeJob.kind} ${data.activeJob.backend_type}`;
		}
	});

	async function refreshBackends() {
		try {
			const result = await listBackends();
			allBackends = [...(result.backends ?? []), ...(result.custom ?? [])];
			availableBackends = result.available ?? [];
			if (result.active_job?.id) {
				activeJobId = result.active_job.id;
				jobTitle = `${result.active_job.kind} ${result.active_job.backend_type}`;
			} else {
				activeJobId = null;
			}
		} catch (e: any) {
			console.warn('Failed to refresh backends:', e);
		}
	}

	async function handleInstall(type: string) {
		installBackendType = type;
		showInstallModal = true;
	}

	async function handleInstallSubmit(request: InstallRequest) {
		error = null;
		try {
			const result = await installBackend(request);
			showInstallModal = false;
			activeJobId = result.job_id;
			jobTitle = `Installing ${request.backend_type}`;
			addToast('Install Started', `Backend install job: ${result.job_id}`, 'success');
			// Refresh after job completes
			setTimeout(refreshBackends, 5000);
		} catch (e: any) {
			const msg = e.message || 'Failed to start install.';
			error = msg;
			addToast('Error', msg, 'error');
		}
	}

	async function handleUpdate(type: string, variant: string) {
		error = null;
		try {
			const result = await updateBackend(type, variant);
			activeJobId = result.job_id;
			jobTitle = `Updating ${type}`;
			addToast('Update Started', `Backend update job: ${result.job_id}`, 'success');
			setTimeout(refreshBackends, 5000);
		} catch (e: any) {
			const msg = e.message || 'Failed to start update.';
			error = msg;
			addToast('Error', msg, 'error');
		}
	}

	async function handleDelete(type: string, variant: string) {
		if (!confirm(`Uninstall ${type}${variant ? ` (${variant})` : ''}?`)) return;
		error = null;
		try {
			await removeBackend(type, variant);
			addToast('Success', `Backend ${type} uninstalled.`, 'success');
			await refreshBackends();
		} catch (e: any) {
			const msg = e.message || 'Failed to uninstall.';
			error = msg;
			addToast('Error', msg, 'error');
		}
	}

	async function handleCheckUpdates() {
		checking = true;
		error = null;
		try {
			await checkAllUpdates();
			await new Promise((r) => setTimeout(r, 1000));
			await refreshBackends();
			addToast('Success', 'Update check complete.', 'success');
		} catch (e: any) {
			const msg = e.message || 'Failed to check for updates.';
			error = msg;
			addToast('Error', msg, 'error');
		}
		checking = false;
	}

	async function handleSaveChanges() {
		saving = true;
		error = null;
		try {
			const updates = Array.from(pendingArgs.entries());
			await Promise.all(
				updates.map(async ([key, value]) => {
					const [name] = key.split(':');
					const args = value.trim() ? value.trim().split(/\s+/) : [];
					await updateDefaultArgs(name, args);
				})
			);
			pendingArgs = new Map();
			addToast('Success', 'Default args saved.', 'success');
			await refreshBackends();
		} catch (e: any) {
			const msg = e.message || 'Failed to save changes.';
			error = msg;
			addToast('Error', msg, 'error');
		}
		saving = false;
	}

	function handleDefaultArgsChange(key: string, value: string) {
		pendingArgs.set(key, value);
	}

	function handleJobDone() {
		setTimeout(refreshBackends, 1000);
	}

	function hasPendingChanges(): boolean {
		return pendingArgs.size > 0;
	}
</script>

<div class="page">
	<div class="page-header">
		<h1>&#128295; Backends</h1>
		<div class="page-header-actions">
			<button class="btn btn-secondary" onclick={handleCheckUpdates} disabled={checking}>
				{checking ? 'Checking...' : 'Check for updates'}
			</button>
			<button
				class="btn btn-primary"
				onclick={handleSaveChanges}
				disabled={!hasPendingChanges() || saving}
			>
				{saving ? 'Saving...' : 'Save Changes'}
			</button>

			<!-- Add Backend dropdown -->
			<div class="relative">
				<button
					class="btn btn-primary"
					onclick={() => showAddDropdown = !showAddDropdown}
				>
					+ Add Backend
				</button>
				{#if showAddDropdown}
					<div class="absolute right-0 mt-1 w-48 bg-bg-secondary border border-border-default rounded-lg shadow-lg z-10 overflow-hidden">
						{#each knownBackends as kb}
							<button
								class="w-full text-left px-3 py-2 text-sm text-text-primary hover:bg-bg-tertiary transition-colors"
								onclick={() => {
									showAddDropdown = false;
									handleInstall(kb.type);
								}}
							>
								{kb.label}
							</button>
						{/each}
					</div>
				{/if}
			</div>
		</div>
	</div>

	<!-- Error banner -->
	{#if error}
		<div class="mb-4 rounded-md bg-accent-red/20 px-4 py-3 text-sm text-accent-red">{error}</div>
	{/if}

	<!-- Job log panel -->
	{#if activeJobId}
		<div class="mb-6">
			<JobLogPanel
				jobId={activeJobId ?? ''}
				title={jobTitle}
				onClose={() => (activeJobId = null)}
				onDone={handleJobDone}
			/>
		</div>
	{/if}

	<!-- System capabilities -->
	<section class="mb-8">
		<h2 class="mb-3 text-lg font-semibold text-text-primary">System Capabilities</h2>
		<div class="card">
			<div class="grid grid-cols-2 md:grid-cols-4 gap-4 text-sm">
				<div>
					<span class="text-text-muted">OS:</span>
					<span class="ml-2 text-text-primary font-medium">{capabilities.os}</span>
				</div>
				<div>
					<span class="text-text-muted">Arch:</span>
					<span class="ml-2 text-text-primary font-medium">{capabilities.arch}</span>
				</div>
				<div>
					<span class="text-text-muted">CUDA:</span>
					<span class="ml-2 text-text-primary font-medium">
						{capabilities.detected_cuda_version || 'Not detected'}
					</span>
				</div>
				<div>
					<span class="text-text-muted">Build tools:</span>
					<span class="ml-2">
						<span class="badge {capabilities.git_available ? 'badge-success' : 'badge-danger'}">
							git
						</span>
						<span class="badge {capabilities.cmake_available ? 'badge-success' : 'badge-danger'} ml-1">
							cmake
						</span>
						<span class="badge {capabilities.compiler_available ? 'badge-success' : 'badge-danger'} ml-1">
							compiler
						</span>
					</span>
				</div>
			</div>
		</div>
	</section>

	<!-- Backend cards -->
	<section>
		<h2 class="mb-3 text-lg font-semibold text-text-primary">Installed Backends</h2>

		{#if allBackends.length === 0}
			<div class="card flex flex-col items-center justify-center py-12 gap-3">
				<p class="text-muted">No backends installed</p>
				<button class="btn btn-primary" onclick={() => handleInstall('llama_cpp')}>
					+ Install llama.cpp
				</button>
			</div>
		{:else}
			<div class="flex flex-col gap-3">
				{#each allBackends as backend (backend.type + backend.gpu_variant)}
					<BackendCard
						{backend}
						onInstall={handleInstall}
						onUpdate={handleUpdate}
						onCheckUpdates={handleCheckUpdates}
						onDelete={handleDelete}
						onDefaultArgsChange={handleDefaultArgsChange}
					/>
				{/each}
			</div>
		{/if}
	</section>
</div>

<!-- Install Modal -->
<InstallModal
	open={showInstallModal}
	backendType={installBackendType}
	{capabilities}
	onSubmit={handleInstallSubmit}
	onClose={() => (showInstallModal = false)}
/>

<!-- Click outside to close dropdown -->
{#if showAddDropdown}
	<div class="fixed inset-0 z-0" onclick={() => (showAddDropdown = false)}></div>
{/if}
