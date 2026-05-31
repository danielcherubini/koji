import { api } from '$lib/api/client';
import type { Alias, ModelOption, UpdateAliasForm } from '$lib/types/aliases';

/** Fetch all aliases */
export async function listAliases(): Promise<Alias[]> {
	const res = await api.get('/aliases');
	if (!res.ok) throw new Error(`Failed to fetch aliases: ${res.status}`);
	return res.json();
}

/** Fetch available models for the dropdown selector */
export async function fetchModels(): Promise<ModelOption[]> {
	const res = await api.get('/models');
	if (!res.ok) throw new Error(`Failed to fetch models: ${res.status}`);
	const data = await res.json();
	const models = data?.models ?? [];
	return models.map((entry: any) => ({
		id: entry.id,
		label: entry.display_name || entry.api_name || entry.repo_id || 'Unknown'
	}));
}

/** Create a new alias */
export async function createAlias(
	name: string,
	model_id: number,
	description: string
): Promise<Alias> {
	const body: any = { name, model_id };
	if (description) body.description = description;
	else body.description = null;
	const res = await api.post('/aliases', body);
	if (!res.ok) {
		const text = await res.text();
		throw new Error(`Failed to create alias: ${res.status} ${text}`);
	}
	return res.json();
}

/** Update an existing alias */
export async function updateAlias(
	id: number,
	data: UpdateAliasForm
): Promise<Alias> {
	const body: any = {};
	if (data.name !== undefined) body.name = data.name;
	if (data.model_id !== undefined) body.model_id = data.model_id;
	if (data.description !== undefined) body.description = data.description || null;
	if (data.enabled !== undefined) body.enabled = data.enabled;
	const res = await api.put(`/aliases/${id}`, body);
	if (!res.ok) {
		const text = await res.text();
		throw new Error(`Failed to update alias: ${res.status} ${text}`);
	}
	return res.json();
}

/** Delete an alias */
export async function deleteAlias(id: number): Promise<void> {
	const res = await api.delete(`/aliases/${id}`);
	if (!res.ok) throw new Error(`Failed to delete alias: ${res.status}`);
}
