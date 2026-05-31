import { api } from '$lib/api/client';
import type { ModelEntry, ModelsResponse } from '$lib/types/models';

/** Fetch all models */
export async function listModels(): Promise<ModelsResponse> {
	const res = await api.get('/models');
	if (!res.ok) throw new Error(`Failed to fetch models: ${res.status}`);
	return res.json();
}

/** Load a model by ID */
export async function loadModel(id: number): Promise<void> {
	const res = await api.post(`/models/${id}/load`);
	if (!res.ok) {
		const text = await res.text();
		throw new Error(`Failed to load model: ${res.status} ${text}`);
	}
}

/** Unload a model by ID */
export async function unloadModel(id: number): Promise<void> {
	const res = await api.post(`/models/${id}/unload`);
	if (!res.ok) {
		const text = await res.text();
		throw new Error(`Failed to unload model: ${res.status} ${text}`);
	}
}

/** Refresh metadata for a single model */
export async function refreshModel(id: number): Promise<void> {
	const res = await api.post(`/models/${id}/refresh`);
	if (!res.ok) {
		const text = await res.text();
		throw new Error(`Failed to refresh model: ${res.status} ${text}`);
	}
}

/** Refresh metadata for all models */
export async function refreshAllModels(): Promise<void> {
	const data = await listModels();
	const refreshPromises = data.models.map((model: ModelEntry) =>
		refreshModel(model.id).catch((err) => {
			console.warn(`Failed to refresh model ${model.id}:`, err);
		})
	);
	await Promise.all(refreshPromises);
}
