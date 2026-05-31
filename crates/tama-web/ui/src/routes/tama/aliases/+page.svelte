<script lang="ts">
	import { createAlias, updateAlias, deleteAlias } from '$lib/api/aliases';
	import type { Alias, ModelOption } from '$lib/types/aliases';
	import Modal from '$lib/components/Modal.svelte';
	import { addToast } from '$lib/stores/toasts';

	let { data } = $props();

	let aliases = $state<Alias[]>(data.aliases ?? []);
	let models = $state<ModelOption[]>(data.models ?? []);

	let showCreateModal = $state(false);
	let editingAlias = $state<Alias | null>(null);
	let saveError = $state<string | null>(null);

	// Create form state
	let createName = $state('');
	let createModelId = $state<number>(models[0]?.id ?? 0);
	let createDescription = $state('');

	// Edit form state
	let editName = $state('');
	let editModelId = $state(0);
	let editDescription = $state('');
	let editEnabled = $state(true);

	function openCreateModal() {
		createName = '';
		createModelId = models[0]?.id ?? 0;
		createDescription = '';
		saveError = null;
		showCreateModal = true;
	}

	function openEditModal(alias: Alias) {
		editName = alias.name;
		editModelId = alias.model_id;
		editDescription = alias.description ?? '';
		editEnabled = alias.enabled;
		saveError = null;
		editingAlias = alias;
	}

	function validateName(name: string): string | null {
		if (!name) return 'Alias name is required.';
		if (name.length > 128) return 'Alias name must be 128 characters or fewer.';
		if (!/^[a-zA-Z0-9]/.test(name)) return 'Alias name must start with a letter or number.';
		if (!/^[a-zA-Z0-9][a-zA-Z0-9_-]{0,127}$/.test(name))
			return 'Alias name can only contain letters, numbers, hyphens, and underscores.';
		return null;
	}

	async function handleCreate() {
		saveError = null;
		const nameErr = validateName(createName.trim());
		if (nameErr) { saveError = nameErr; return; }
		if (!createModelId) { saveError = 'Please select a model.'; return; }

		try {
			const newAlias = await createAlias(createName.trim(), createModelId, createDescription.trim());
			aliases = [...aliases, newAlias].sort((a, b) => a.name.localeCompare(b.name));
			showCreateModal = false;
			addToast('Success', 'Alias created successfully.', 'success');
		} catch (e: any) {
			saveError = e.message || 'Failed to create alias.';
		}
	}

	async function handleUpdate() {
		saveError = null;
		if (!editingAlias) return;
		const nameErr = validateName(editName.trim());
		if (nameErr) { saveError = nameErr; return; }
		if (!editModelId) { saveError = 'Please select a model.'; return; }

		try {
			const updated = await updateAlias(editingAlias.id, {
				name: editName.trim(),
				model_id: editModelId,
				description: editDescription.trim() || null,
				enabled: editEnabled
			});
			aliases = aliases
				.map((a) => (a.id === updated.id ? updated : a))
				.sort((a, b) => a.name.localeCompare(b.name));
			editingAlias = null;
			addToast('Success', 'Alias updated successfully.', 'success');
		} catch (e: any) {
			saveError = e.message || 'Failed to update alias.';
		}
	}

	async function handleToggle(alias: Alias) {
		try {
			const updated = await updateAlias(alias.id, { enabled: !alias.enabled });
			aliases = aliases.map((a) => (a.id === updated.id ? updated : a));
			addToast('Success', `Alias ${updated.enabled ? 'enabled' : 'disabled'}.`, 'success');
		} catch (e: any) {
			addToast('Error', `Failed to toggle alias: ${e.message}`, 'error');
		}
	}

	async function handleDelete(alias: Alias) {
		if (!confirm(`Delete alias "${alias.name}"? This cannot be undone.`)) return;
		try {
			await deleteAlias(alias.id);
			aliases = aliases.filter((a) => a.id !== alias.id);
			addToast('Success', `Alias "${alias.name}" deleted.`, 'success');
		} catch (e: any) {
			addToast('Error', `Failed to delete alias: ${e.message}`, 'error');
		}
	}
</script>

<div class="page">
	<div class="page-header">
		<h1>&#127991;&#65039; Aliases</h1>
		<button class="btn btn-primary" onclick={openCreateModal}>+ New Alias</button>
	</div>

	{#if aliases.length === 0}
		<div class="card flex items-center justify-center py-12">
			<p class="text-muted">No aliases yet. Click + New to create one.</p>
		</div>
	{:else}
		<div class="flex flex-col gap-3">
			{#each aliases as alias (alias.id)}
				<div class="card flex items-center justify-between">
					<div class="flex items-center gap-3">
						<span
							class="inline-block h-2.5 w-2.5 rounded-full"
							class:bg-accent-green={alias.enabled}
							class:bg-text-muted={!alias.enabled}
						></span>
						<div>
							<span class="font-medium text-text-primary">{alias.name}</span>
							<span class="text-secondary ml-2">&rarr; {alias.model_name}</span>
							{#if alias.description}
								<span class="text-muted ml-2 text-sm">{alias.description}</span>
							{/if}
						</div>
					</div>
					<div class="flex items-center gap-1">
						<span
							class="badge"
							class:badge-success={alias.enabled}
							class:badge-danger={!alias.enabled}
						>
							{alias.enabled ? 'Enabled' : 'Disabled'}
						</span>
						<button
							class="btn btn-secondary btn-sm"
							title="Edit"
							onclick={() => openEditModal(alias)}
						>&#9998;</button>
						<button
							class="btn btn-secondary btn-sm"
							title={alias.enabled ? 'Disable' : 'Enable'}
							onclick={() => handleToggle(alias)}
						>
							{alias.enabled ? '&#128065;' : '&#128683;'}
						</button>
						<button
							class="btn btn-danger btn-sm"
							title="Delete"
							onclick={() => handleDelete(alias)}
						>&#128465;</button>
					</div>
				</div>
			{/each}
		</div>
	{/if}
</div>

<!-- Create Modal -->
<Modal open={showCreateModal} onClose={() => (showCreateModal = false)} title="Create Alias">
	<form onsubmit={(e) => { e.preventDefault(); handleCreate(); }}>
		<div class="mb-4">
			<label class="mb-1 block text-sm font-medium text-text-primary" for="create-name">Alias Name</label>
			<input
				id="create-name"
				type="text"
				class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
				placeholder="e.g. my-fast-model"
				bind:value={createName}
				autofocus
			/>
			<p class="mt-1 text-xs text-text-muted">Must start with a letter or number, max 128 chars. Allowed: a-z, 0-9, -, _</p>
		</div>
		<div class="mb-4">
			<label class="mb-1 block text-sm font-medium text-text-primary" for="create-model">Model</label>
			<select
				id="create-model"
				class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
				bind:value={createModelId}
			>
				<option value={0}>-- Select a model --</option>
				{#each models as m (m.id)}
					<option value={m.id}>{m.label}</option>
				{/each}
			</select>
		</div>
		<div class="mb-4">
			<label class="mb-1 block text-sm font-medium text-text-primary" for="create-desc">Description (optional)</label>
			<textarea
				id="create-desc"
				class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
				placeholder="What is this alias for?"
				bind:value={createDescription}
				rows={3}
			></textarea>
		</div>
		{#if saveError}
			<div class="mb-4 rounded-md bg-accent-red/20 px-3 py-2 text-sm text-accent-red">{saveError}</div>
		{/if}
		<div class="flex justify-end gap-2">
			<button type="button" class="btn btn-secondary" onclick={() => (showCreateModal = false)}>Cancel</button>
			<button type="submit" class="btn btn-primary">Create Alias</button>
		</div>
	</form>
</Modal>

<!-- Edit Modal -->
<Modal open={editingAlias !== null} onClose={() => (editingAlias = null)} title={`Edit: ${editingAlias?.name ?? ''}`}>
	<form onsubmit={(e) => { e.preventDefault(); handleUpdate(); }}>
		<div class="mb-4">
			<label class="mb-1 block text-sm font-medium text-text-primary" for="edit-name">Alias Name</label>
			<input
				id="edit-name"
				type="text"
				class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
				placeholder="e.g. my-fast-model"
				bind:value={editName}
				autofocus
			/>
		</div>
		<div class="mb-4">
			<label class="mb-1 block text-sm font-medium text-text-primary" for="edit-model">Model</label>
			<select
				id="edit-model"
				class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
				bind:value={editModelId}
			>
				<option value={0}>-- Select a model --</option>
				{#each models as m (m.id)}
					<option value={m.id}>{m.label}</option>
				{/each}
			</select>
		</div>
		<div class="mb-4">
			<label class="mb-1 block text-sm font-medium text-text-primary" for="edit-desc">Description (optional)</label>
			<textarea
				id="edit-desc"
				class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
				placeholder="What is this alias for?"
				bind:value={editDescription}
				rows={3}
			></textarea>
		</div>
		<div class="mb-4 flex items-center gap-2">
			<input
				id="edit-enabled"
				type="checkbox"
				class="rounded border-border-default bg-bg-primary text-accent-blue focus:ring-accent-blue/50"
				bind:checked={editEnabled}
			/>
			<label class="text-sm text-text-primary" for="edit-enabled">Enabled</label>
		</div>
		{#if saveError}
			<div class="mb-4 rounded-md bg-accent-red/20 px-3 py-2 text-sm text-accent-red">{saveError}</div>
		{/if}
		<div class="flex justify-end gap-2">
			<button type="button" class="btn btn-secondary" onclick={() => (editingAlias = null)}>Cancel</button>
			<button type="submit" class="btn btn-primary">Save Changes</button>
		</div>
	</form>
</Modal>
