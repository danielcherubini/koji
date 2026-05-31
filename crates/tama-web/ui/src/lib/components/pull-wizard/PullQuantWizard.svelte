<script lang="ts">
	import Modal from '$lib/components/Modal.svelte';
	import RepoInput from './RepoInput.svelte';
	import LoadingStep from './LoadingStep.svelte';
	import SelectionStep from './SelectionStep.svelte';
	import DownloadStep from './DownloadStep.svelte';
	import SetContextStep from './SetContextStep.svelte';
	import DoneStep from './DoneStep.svelte';
	import { fetchHfMetadata, startPull, getPullProgress } from '$lib/api/pull-wizard';
	import type {
		HfMetadata,
		HfFileInfo,
		DownloadProgress,
		CompletedQuant
	} from '$lib/api/pull-wizard';

	interface Props {
		open: boolean;
		initialRepo: string;
		onClose: () => void;
		onComplete: (completed: CompletedQuant[]) => void;
	}

	let { open, initialRepo, onClose, onComplete }: Props = $props();

	// Wizard step: 'repo' | 'loading' | 'selection' | 'download' | 'context' | 'done'
	type WizardStep = 'repo' | 'loading' | 'selection' | 'download' | 'context' | 'done';
	let currentStep = $state<WizardStep>('repo');

	// Repo state
	let repoId = $state('');
	let hfMetadata = $state<HfMetadata | null>(null);

	// Selection state
	let selectedFiles = $state<string[]>([]);

	// Download state
	let downloadProgress = $state<DownloadProgress[]>([]);
	let pollInterval: ReturnType<typeof setInterval> | null = null;

	// Context settings
	let contextLength = $state<number | undefined>(undefined);
	let kvUnified = $state(true);
	let cacheTypeK = $state<string | undefined>(undefined);
	let cacheTypeV = $state<string | undefined>(undefined);

	// Completed results
	let completedQuants = $state<CompletedQuant[]>([]);

	// Error state
	let error = $state<string | null>(null);

	function resetWizard() {
		currentStep = 'repo';
		repoId = '';
		hfMetadata = null;
		selectedFiles = [];
		downloadProgress = [];
		contextLength = undefined;
		kvUnified = true;
		cacheTypeK = undefined;
		cacheTypeV = undefined;
		completedQuants = [];
		error = null;
		if (pollInterval) {
			clearInterval(pollInterval);
			pollInterval = null;
		}
	}

	function handleClose() {
		resetWizard();
		onClose();
	}

	async function handleLoadRepo(id: string) {
		repoId = id;
		currentStep = 'loading';
		error = null;

		try {
			const metadata = await fetchHfMetadata(id);
			if (!metadata) {
				error = 'Repo not found or no files available.';
				currentStep = 'repo';
				return;
			}
			hfMetadata = metadata;

			// Auto-select all quants
			selectedFiles = [
				...metadata.quants.map((q) => q.filename),
				...metadata.mmprojs.map((m) => m.filename)
			];

			// Auto-set context length from metadata
			if (metadata.context_length) {
				contextLength = metadata.context_length;
			}

			currentStep = 'selection';
		} catch (e: unknown) {
			const msg = e instanceof Error ? e.message : 'Failed to load repo metadata';
			error = msg;
			currentStep = 'repo';
		}
	}

	function toggleFile(filename: string) {
		const idx = selectedFiles.indexOf(filename);
		if (idx >= 0) {
			selectedFiles = selectedFiles.filter((f) => f !== filename);
		} else {
			selectedFiles = [...selectedFiles, filename];
		}
	}

	function selectAll() {
		if (!hfMetadata) return;
		selectedFiles = [
			...hfMetadata.quants.map((q) => q.filename),
			...hfMetadata.mmprojs.map((m) => m.filename)
		];
	}

	function deselectAll() {
		selectedFiles = [];
	}

	async function handleDownload() {
		if (selectedFiles.length === 0) return;
		currentStep = 'download';
		error = null;
		downloadProgress = selectedFiles.map((f) => ({
			filename: f,
			bytes_downloaded: 0,
			total_bytes: null,
			status: 'queued'
		}));

		try {
			const response = await startPull({
				repo_id: repoId,
				files: selectedFiles
			});

			const jobId = response.job_id;

			// Poll for progress
			pollInterval = setInterval(async () => {
				try {
					const progress = await getPullProgress(jobId);
					downloadProgress = progress;

					// Check if all done
					const allDone = progress.every(
						(p) => ['completed', 'failed', 'cancelled'].includes(p.status)
					);
					if (allDone) {
						if (pollInterval) {
							clearInterval(pollInterval);
							pollInterval = null;
						}

						// Build completed list
						const completed: CompletedQuant[] = progress
							.filter((p) => p.status === 'completed')
							.map((p) => {
								const allFiles = [...(hfMetadata?.quants ?? []), ...(hfMetadata?.mmprojs ?? [])];
								const info = allFiles.find((f) => f.filename === p.filename);
								return {
									filename: p.filename,
									quant: info?.quant,
									size_bytes: p.total_bytes ?? info?.size
								};
							});

						if (completed.length > 0) {
							completedQuants = completed;
							currentStep = 'context';
						} else {
							completedQuants = [];
							currentStep = 'done';
						}
					}
				} catch {
					// Polling errors are non-fatal
				}
			}, 1000);
		} catch (e: unknown) {
			const msg = e instanceof Error ? e.message : 'Failed to start download';
			error = msg;
			currentStep = 'selection';
		}
	}

	function handleCancelDownload() {
		if (pollInterval) {
			clearInterval(pollInterval);
			pollInterval = null;
		}
		currentStep = 'selection';
		downloadProgress = [];
	}

	function handleFinish() {
		if (pollInterval) {
			clearInterval(pollInterval);
			pollInterval = null;
		}
		onComplete(completedQuants);
		currentStep = 'done';
	}

	// Cleanup on close
	$effect(() => {
		if (!open) {
			if (pollInterval) {
				clearInterval(pollInterval);
				pollInterval = null;
			}
		}
	});
</script>

<Modal open={open} onClose={handleClose} title="Pull Quant from HuggingFace">
	<div class="min-w-[28rem]">
		{#if error}
			<div class="mb-4 rounded-md bg-accent-red/20 px-3 py-2 text-sm text-accent-red">{error}</div>
		{/if}

		{#if currentStep === 'repo'}
			<RepoInput initialRepo={initialRepo} onLoad={handleLoadRepo} />
		{:else if currentStep === 'loading'}
			<LoadingStep repoId={repoId} />
		{:else if currentStep === 'selection' && hfMetadata}
			<SelectionStep
				repoId={repoId}
				quants={hfMetadata.quants}
				mmprojs={hfMetadata.mmprojs}
				{selectedFiles}
				onToggle={toggleFile}
				onSelectAll={selectAll}
				onDeselectAll={deselectAll}
				onDownload={handleDownload}
				onBack={() => (currentStep = 'repo')}
			/>
		{:else if currentStep === 'download'}
			<DownloadStep progress={downloadProgress} onCancel={handleCancelDownload} />
		{:else if currentStep === 'context'}
			<SetContextStep
				{contextLength}
				{kvUnified}
				{cacheTypeK}
				{cacheTypeV}
				onContextLengthChange={(val) => (contextLength = val)}
				onKvUnifiedChange={(val) => (kvUnified = val)}
				onCacheTypeKChange={(val) => (cacheTypeK = val)}
				onCacheTypeVChange={(val) => (cacheTypeV = val)}
				onFinish={handleFinish}
			/>
		{:else if currentStep === 'done'}
			<DoneStep completed={completedQuants} onClose={handleClose} />
		{/if}
	</div>
</Modal>
