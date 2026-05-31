<script lang="ts">
	import type { ModelForm, QuantInfo } from '$lib/types/model-editor';
	import { formatSize } from '$lib/utils/formatting';

	interface Props {
		form: ModelForm;
		repoCommitSha: string | undefined;
		repoPulledAt: string | undefined;
		refreshBusy: boolean;
		verifyBusy: boolean;
		refreshStatus: { ok: boolean; message: string } | null;
		verifyStatus: { ok: boolean; message: string } | null;
		pullModalOpen: boolean;
		onRefresh: () => void;
		onVerify: () => void;
		onDeleteQuant: (key: string) => void;
		onOpenPullWizard: () => void;
		onSetPullModalOpen: (open: boolean) => void;
	}

	let {
		form,
		repoCommitSha,
		repoPulledAt,
		refreshBusy,
		verifyBusy,
		refreshStatus,
		verifyStatus,
		pullModalOpen,
		onRefresh,
		onVerify,
		onDeleteQuant,
		onOpenPullWizard,
		onSetPullModalOpen
	}: Props = $props();

	let quantEntries = $derived(Object.entries(form.quants ?? {}));

	function getKindLabel(kind: string): string {
		return kind === 'mmproj' ? 'MMProj' : 'Model';
	}

	function getKindBadge(kind: string): string {
		return kind === 'mmproj' ? 'badge-info' : 'badge-success';
	}

	function getVerifyBadge(quant: QuantInfo): string | null {
		if (quant.verified_ok === undefined) return null;
		return quant.verified_ok ? 'badge-success' : 'badge-danger';
	}

	function getVerifyLabel(quant: QuantInfo): string | null {
		if (quant.verified_ok === undefined) return null;
		return quant.verified_ok ? 'Verified' : 'Failed';
	}

	function formatVerifiedAt(isoString: string | undefined): string {
		if (!isoString) return '';
		try {
			return new Date(isoString).toLocaleString();
		} catch {
			return '';
		}
	}
</script>

<div class="space-y-4">
	<!-- Repo metadata -->
	{#if repoCommitSha}
		<div class="text-xs text-text-muted space-y-1">
			{#if repoCommitSha}
				<p>
					<strong>Commit:</strong>
					<code class="bg-bg-tertiary px-1 py-0.5 rounded">{repoCommitSha.slice(0, 12)}</code>
				</p>
			{/if}
			{#if repoPulledAt}
				<p><strong>Pulled:</strong> {formatVerifiedAt(repoPulledAt)}</p>
			{/if}
		</div>
	{/if}

	<!-- Action buttons -->
	<div class="flex flex-wrap items-center gap-2">
		<button
			class="btn btn-secondary btn-sm"
			onclick={onRefresh}
			disabled={refreshBusy}
		>
			{refreshBusy ? 'Refreshing...' : '&#8635; Refresh'}
		</button>
		<button
			class="btn btn-secondary btn-sm"
			onclick={onVerify}
			disabled={verifyBusy}
		>
			{verifyBusy ? 'Verifying...' : '&#10003; Verify'}
		</button>
		<button
			class="btn btn-primary btn-sm"
			onclick={onOpenPullWizard}
		>
			&#11015; Pull Quant
		</button>
	</div>

	<!-- Refresh/Verify status -->
	{#if refreshStatus}
		<div class="rounded-md px-3 py-2 text-sm {refreshStatus.ok ? 'bg-accent-green/20 text-accent-green' : 'bg-accent-red/20 text-accent-red'}">
			{refreshStatus.message}
		</div>
	{/if}

	{#if verifyStatus}
		<div class="rounded-md px-3 py-2 text-sm {verifyStatus.ok ? 'bg-accent-green/20 text-accent-green' : 'bg-accent-red/20 text-accent-red'}">
			{verifyStatus.message}
		</div>
	{/if}

	<!-- Quants table -->
	{#if quantEntries.length === 0}
		<div class="text-sm text-text-muted">No quants configured yet. Use "Pull Quant" to add one.</div>
	{:else}
		<div class="overflow-x-auto">
			<table class="w-full text-sm">
				<thead>
					<tr class="border-b border-border-default text-text-secondary">
						<th class="text-left pb-2 pr-4 font-medium">File</th>
						<th class="text-left pb-2 pr-4 font-medium">Kind</th>
						<th class="text-left pb-2 pr-4 font-medium">Size</th>
						<th class="text-left pb-2 pr-4 font-medium">Context</th>
						<th class="text-left pb-2 pr-4 font-medium">Verified</th>
						<th class="text-right pb-2 font-medium">Actions</th>
					</tr>
				</thead>
				<tbody>
					{#each quantEntries as [key, quant] (key)}
						<tr class="border-b border-border-default/50 hover:bg-bg-tertiary/50">
							<td class="py-2 pr-4">
								<div class="font-medium text-text-primary">{quant.file}</div>
								{#if quant.lfs_oid}
									<div class="text-xs text-text-muted font-mono">{quant.lfs_oid.slice(0, 12)}</div>
								{/if}
							</td>
							<td class="py-2 pr-4">
								<span class="badge {getKindBadge(quant.kind)}">{getKindLabel(quant.kind)}</span>
							</td>
							<td class="py-2 pr-4 text-text-secondary">
								{quant.size_bytes ? formatSize(quant.size_bytes) : '—'}
							</td>
							<td class="py-2 pr-4 text-text-secondary">
								{quant.context_length ?? '—'}
							</td>
							<td class="py-2 pr-4">
								{#if quant.verified_ok !== undefined}
									<span class="badge {getVerifyBadge(quant)}">{getVerifyLabel(quant)}</span>
								{:else}
									<span class="text-text-muted">—</span>
								{/if}
								{#if quant.last_verified_at}
									<div class="text-xs text-text-muted mt-0.5">
										{formatVerifiedAt(quant.last_verified_at)}
									</div>
								{/if}
							</td>
							<td class="py-2 text-right">
								<button
									class="btn btn-danger btn-sm"
									title="Delete quant"
									onclick={() => onDeleteQuant(key)}
								>
									&#128465;
								</button>
							</td>
						</tr>
						{#if quant.verify_error}
							<tr>
								<td colspan="6" class="py-1 pl-4 text-xs text-accent-red">
									{quant.verify_error}
								</td>
							</tr>
						{/if}
					{/each}
				</tbody>
			</table>
		</div>
	{/if}
</div>
