<script lang="ts">
	import { saveModel, renameModel, deleteModel, deleteQuant, refreshModel, verifyModel } from '$lib/api/model-editor';
	import type { ModelForm, BackendOption } from '$lib/types/model-editor';
	import type { CompletedQuant } from '$lib/api/pull-wizard';
	import GeneralForm from '$lib/components/model-editor/GeneralForm.svelte';
	import SamplingForm from '$lib/components/model-editor/SamplingForm.svelte';
	import SpecDecodingForm from '$lib/components/model-editor/SpecDecodingForm.svelte';
	import QuantsVisionForm from '$lib/components/model-editor/QuantsVisionForm.svelte';
	import ExtraArgsForm from '$lib/components/model-editor/ExtraArgsForm.svelte';
	import PullQuantWizard from '$lib/components/pull-wizard/PullQuantWizard.svelte';
	import { addToast } from '$lib/stores/toasts';

	interface PageData {
		id: string;
		form: ModelForm | null;
		backends: BackendOption[];
		samplingTemplates: Record<string, Record<string, unknown>> | null;
		repoCommitSha: string | undefined;
		repoPulledAt: string | undefined;
		isNew: boolean;
	}

	let { data }: { data: PageData } = $props();

	// Form state — initialized from loader data
	let form = $state<ModelForm | null>(data.form);
	let backends = $state<BackendOption[]>(data.backends);
	let samplingTemplates = $state(data.samplingTemplates);

	// Original (persisted) ID for rename tracking
	let originalId = $state(data.id);

	// UI state
	let activeSection = $state<'General' | 'Sampling' | 'Spec Decoding' | 'Quants & Vision' | 'Extra Args'>('General');
	let saveStatus = $state<{ ok: boolean; message: string } | null>(null);
	let deleted = $state(false);
	let formReady = $state(!!data.form);

	// Pull wizard
	let pullModalOpen = $state(false);

	// Refresh/verify state
	let refreshBusy = $state(false);
	let verifyBusy = $state(false);
	let refreshStatus = $state<{ ok: boolean; message: string } | null>(null);
	let verifyStatus = $state<{ ok: boolean; message: string } | null>(null);

	// Repo metadata
	let repoCommitSha = $state(data.repoCommitSha);
	let repoPulledAt = $state(data.repoPulledAt);

	// Derived display title
	let pageTitle = $derived(
		data.isNew
			? 'New Model'
			: form?.display_name?.trim()
				? form.display_name
				: data.id
	);

	// Section definitions for side nav
	const sections = [
		{ key: 'General', icon: '&#9881;', id: 'section-general' },
		{ key: 'Sampling', icon: '&#127176;', id: 'section-sampling' },
		{ key: 'Spec Decoding', icon: '&#9889;', id: 'section-spec-decoding' },
		{ key: 'Quants & Vision', icon: '&#128202;', id: 'section-quants' },
		{ key: 'Extra Args', icon: '&#128221;', id: 'section-extra-args' }
	] as const;

	function scrollToSection(sectionId: string) {
		const el = document.getElementById(sectionId);
		if (el) {
			el.scrollIntoView({ behavior: 'smooth', block: 'start' });
		}
	}

	async function handleSave() {
		if (!form) {
			saveStatus = { ok: false, message: 'Form not loaded.' };
			return;
		}

		saveStatus = null;

		try {
			const args = form.args
				.split('\n')
				.map((l) => l.trim())
				.filter((l) => l.length > 0);

			const saveId = form.id.trim() || originalId;

			// Handle rename if ID changed
			if (originalId && saveId !== originalId && !data.isNew) {
				try {
					await renameModel(originalId, saveId);
				} catch (e: unknown) {
					const msg = e instanceof Error ? e.message : 'Rename failed';
					saveStatus = { ok: false, message: `Rename failed: ${msg}` };
					return;
				}
			}

			await saveModel(args, form, data.isNew);

			if (saveId !== originalId) {
				originalId = saveId;
			}

			saveStatus = { ok: true, message: 'Saved' };
			addToast('Success', 'Model saved successfully.', 'success');
		} catch (e: unknown) {
			const msg = e instanceof Error ? e.message : 'Unknown error';
			// Attempt rollback on rename+save failure
			if (originalId && form.id && form.id !== originalId && !data.isNew) {
				try {
					await renameModel(form.id, originalId);
				} catch (rollbackErr: unknown) {
					const rbMsg = rollbackErr instanceof Error ? rollbackErr.message : 'rollback failed';
					saveStatus = { ok: false, message: `Save failed (${msg}), rollback also failed (${rbMsg})` };
					return;
				}
			}
			saveStatus = { ok: false, message: `Error: ${msg}` };
			addToast('Error', `Failed to save: ${msg}`, 'error');
		}
	}

	async function handleDelete() {
		if (!form) return;
		const confirmed = confirm('Delete this model and all its files from disk? This cannot be undone.');
		if (!confirmed) return;

		try {
			await deleteModel(form.id);
			deleted = true;
			addToast('Success', 'Model deleted.', 'success');
		} catch (e: unknown) {
			const msg = e instanceof Error ? e.message : 'Delete failed';
			saveStatus = { ok: false, message: `Delete failed: ${msg}` };
			addToast('Error', `Failed to delete: ${msg}`, 'error');
		}
	}

	async function handleDeleteQuant(quantKey: string) {
		if (!form) return;
		try {
			await deleteQuant(form.id, quantKey);
			// Remove from local state
			const quants = { ...form.quants };
			delete quants[quantKey];
			form = { ...form, quants };
			if (form.quant === quantKey) {
				form = { ...form, quant: undefined };
			}
			if (form.mmproj === quantKey) {
				form = { ...form, mmproj: undefined };
			}
			saveStatus = { ok: true, message: 'Quant deleted from disk.' };
			addToast('Success', 'Quant deleted.', 'success');
		} catch (e: unknown) {
			const msg = e instanceof Error ? e.message : 'Delete failed';
			saveStatus = { ok: false, message: `Delete failed: ${msg}` };
			addToast('Error', `Failed to delete quant: ${msg}`, 'error');
		}
	}

	async function handleRefresh() {
		if (!form) return;
		refreshBusy = true;
		refreshStatus = null;
		const id = originalId || form.id;
		try {
			const resp = await refreshModel(id);
			repoCommitSha = resp.repo_commit_sha;
			repoPulledAt = resp.repo_pulled_at;
			// Merge file records into quants
			if (resp.files.length > 0 && form) {
				const quants = { ...form.quants };
				for (const rec of resp.files) {
					for (const [key, q] of Object.entries(quants)) {
						if (q.file === rec.filename) {
							quants[key] = {
								...q,
								lfs_oid: rec.lfs_oid,
								db_size_bytes: rec.size_bytes,
								size_bytes: rec.size_bytes ?? q.size_bytes,
								last_verified_at: rec.last_verified_at,
								verified_ok: rec.verified_ok,
								verify_error: rec.verify_error
							};
						}
					}
				}
				form = { ...form, quants };
			}
			refreshStatus = { ok: true, message: `Refreshed metadata for ${resp.files.length} file(s).` };
		} catch (e: unknown) {
			const msg = e instanceof Error ? e.message : 'Refresh failed';
			refreshStatus = { ok: false, message: `Refresh failed: ${msg}` };
		}
		refreshBusy = false;
	}

	async function handleVerify() {
		if (!form) return;
		verifyBusy = true;
		verifyStatus = null;
		const id = originalId || form.id;
		try {
			const resp = await verifyModel(id);
			// Merge file records
			if (resp.files.length > 0 && form) {
				const quants = { ...form.quants };
				for (const rec of resp.files) {
					for (const [key, q] of Object.entries(quants)) {
						if (q.file === rec.filename) {
							quants[key] = {
								...q,
								lfs_oid: rec.lfs_oid,
								db_size_bytes: rec.size_bytes,
								size_bytes: rec.size_bytes ?? q.size_bytes,
								last_verified_at: rec.last_verified_at,
								verified_ok: rec.verified_ok,
								verify_error: rec.verify_error
							};
						}
					}
				}
				form = { ...form, quants };
			}
			let msg: string;
			if (resp.ok && !resp.any_unknown) {
				msg = `All ${resp.files.length} file(s) verified successfully.`;
			} else if (resp.ok) {
				msg = `Verified ${resp.files.length} file(s) (some without an upstream hash).`;
			} else {
				msg = 'Verification failed for one or more files.';
			}
			verifyStatus = { ok: resp.ok, message: msg };
		} catch (e: unknown) {
			const msg = e instanceof Error ? e.message : 'Verify failed';
			verifyStatus = { ok: false, message: `Verify failed: ${msg}` };
		}
		verifyBusy = false;
	}

	function handleWizardComplete(completed: CompletedQuant[]) {
		if (!form || completed.length === 0) {
			pullModalOpen = false;
			return;
		}

		const quants = { ...form.quants };
		for (const cq of completed) {
			const lower = cq.filename.toLowerCase();
			const kind = lower.startsWith('mmproj') && lower.endsWith('.gguf') ? 'mmproj' : 'model';

			// Infer quant key from filename
			let quantKey = cq.quant;
			if (!quantKey) {
				const stem = cq.filename.replace(/\.gguf$/, '');
				const patterns = [
					'IQ2_XXS', 'IQ3_XXS', 'IQ1_S', 'IQ1_M', 'IQ2_XS', 'IQ2_S', 'IQ2_M',
					'IQ3_XS', 'IQ3_S', 'IQ3_M', 'IQ4_XS', 'IQ4_NL',
					'Q2_K_S', 'Q3_K_S', 'Q3_K_M', 'Q3_K_L', 'Q4_K_S', 'Q4_K_M',
					'Q4_K_L', 'Q5_K_S', 'Q5_K_M', 'Q5_K_L', 'Q2_K_XL', 'Q3_K_XL',
					'Q4_K_XL', 'Q5_K_XL', 'Q6_K_XL', 'Q8_K_XL', 'Q2_K', 'Q3_K',
					'Q4_K', 'Q5_K', 'Q6_K', 'Q4_0', 'Q4_1', 'Q5_0', 'Q5_1',
					'Q6_0', 'Q8_0', 'Q8_1', 'F16', 'F32', 'BF16'
				];
				const stemUpper = stem.toUpperCase();
				quantKey = patterns.find((p) =>
					stemUpper.endsWith(p) ||
					stemUpper.includes(`-${p}`) ||
					stemUpper.includes(`.${p}`) ||
					stemUpper.includes(`_${p}`)
				) ?? stem.split(/[-_]/).pop() ?? 'unknown';
			}

			if (quantKey in quants) {
				// Re-pull: overwrite
				const existing = quants[quantKey];
				quants[quantKey] = {
					...existing,
					file: cq.filename,
					kind,
					size_bytes: cq.size_bytes ?? existing.size_bytes
				};
			} else {
				quants[quantKey] = {
					file: cq.filename,
					kind,
					size_bytes: cq.size_bytes
				};
			}
		}
		form = { ...form, quants };
		pullModalOpen = false;
		addToast('Success', `${completed.length} quant(s) pulled and added.`, 'success');
	}
</script>

<div class="page">
	<!-- Page header -->
	<div class="page-header">
		<h1>{pageTitle}</h1>
		<div class="page-header-actions">
			{#if saveStatus}
				<span class="text-muted">{saveStatus.message}</span>
			{/if}
			<button class="btn btn-primary" onclick={handleSave}>Save Model</button>
			{#if !data.isNew}
				<button class="btn btn-danger" onclick={handleDelete}>Delete Model</button>
			{/if}
			<a href="/tama/models" class="btn btn-secondary btn-sm">&larr; Back to Models</a>
		</div>
	</div>

	<!-- Deleted state -->
	{#if deleted}
		<div class="card mb-4 flex items-center gap-3 bg-accent-green/10 border-accent-green/30">
			<span>&#10003;</span>
			<span>Model deleted. <a href="/tama/models" class="text-accent-blue hover:underline">&larr; Back to Models</a></span>
		</div>
	{/if}

	<!-- Loading state -->
	{#if !formReady}
		<div class="card flex items-center justify-center gap-3 py-12">
			<div class="h-5 w-5 animate-spin rounded-full border-2 border-text-muted border-t-accent-blue"></div>
			<span class="text-muted">Loading model...</span>
		</div>
	{:else if form}
		<!-- Editor layout -->
		<div class="model-editor-layout">
			<!-- Side navigation -->
			<nav class="model-editor-nav">
				{#each sections as section}
					<button
						class="nav-btn"
						class:nav-btn--active={activeSection === section.key}
						onclick={() => {
							activeSection = section.key;
							scrollToSection(section.id);
						}}
					>
						<span class="nav-btn__icon">{@html section.icon}</span>
						<span class="nav-btn__text">{section.key}</span>
					</button>
				{/each}
			</nav>

			<!-- Main content -->
			<div class="model-editor-main">
				<!-- General Section -->
				<div id="section-general" class="card">
					<h2 class="card__title">General</h2>
					<GeneralForm {form} {backends} />
				</div>

				<!-- Sampling Section -->
				<div id="section-sampling" class="card mt-2">
					<h2 class="card__title">Sampling</h2>
					<SamplingForm {form} {samplingTemplates} />
				</div>

				<!-- Spec Decoding Section -->
				<div id="section-spec-decoding" class="card mt-2">
					<h2 class="card__title">Spec Decoding</h2>
					<SpecDecodingForm {form} />
				</div>

				<!-- Quants & Vision Section -->
				<div id="section-quants" class="card mt-2">
					<h2 class="card__title">Quants & Vision</h2>
					<QuantsVisionForm
						{form}
						{repoCommitSha}
						{repoPulledAt}
						{refreshBusy}
						{verifyBusy}
						{refreshStatus}
						{verifyStatus}
						{pullModalOpen}
						onRefresh={handleRefresh}
						onVerify={handleVerify}
						onDeleteQuant={handleDeleteQuant}
						onOpenPullWizard={() => (pullModalOpen = true)}
						onSetPullModalOpen={(open: boolean) => (pullModalOpen = open)}
					/>
				</div>

				<!-- Extra Args Section -->
				<div id="section-extra-args" class="card mt-2">
					<h2 class="card__title">Extra Args</h2>
					<ExtraArgsForm {form} />
				</div>
			</div>
		</div>
	{/if}
</div>

<!-- Pull Quant Wizard Modal -->
<PullQuantWizard
	open={pullModalOpen}
	initialRepo={form?.model ?? ''}
	onClose={() => (pullModalOpen = false)}
	onComplete={handleWizardComplete}
/>

<style>
	.model-editor-layout {
		display: flex;
		gap: 1.5rem;
		align-items: flex-start;
	}

	.model-editor-nav {
		position: sticky;
		top: 1rem;
		display: flex;
		flex-direction: column;
		gap: 0.25rem;
		width: 14rem;
		shrink: 0;
	}

	.nav-btn {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		padding: 0.5rem 0.75rem;
		border: 1px solid transparent;
		border-radius: 0.5rem;
		background: transparent;
		color: var(--color-text-secondary);
		font-size: 0.875rem;
		cursor: pointer;
		transition: all 0.15s;
		text-align: left;
	}

	.nav-btn:hover {
		background: var(--color-bg-tertiary);
		color: var(--color-text-primary);
	}

	.nav-btn--active {
		background: var(--color-bg-tertiary);
		border-color: var(--color-accent-blue);
		color: var(--color-accent-blue);
	}

	.nav-btn__icon {
		font-size: 1rem;
		line-height: 1;
	}

	.nav-btn__text {
		white-space: nowrap;
	}

	.model-editor-main {
		flex: 1;
		min-width: 0;
	}

	.card__title {
		font-size: 1.125rem;
		font-weight: 600;
		color: var(--color-text-primary);
		margin: 0 0 1rem 0;
	}

	@media (max-width: 768px) {
		.model-editor-layout {
			flex-direction: column;
		}

		.model-editor-nav {
			position: static;
			width: 100%;
			flex-direction: row;
			flex-wrap: wrap;
			gap: 0.25rem;
		}

		.nav-btn {
			flex: 1;
			min-width: 0;
			justify-content: center;
			padding: 0.375rem 0.5rem;
			font-size: 0.75rem;
		}

		.nav-btn__icon {
			display: none;
		}
	}
</style>
