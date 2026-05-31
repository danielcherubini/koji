import { api } from '$lib/api/client';
import type { UpdatesListResponse } from '$lib/types/updates';

/** Fetch all updates status */
export async function fetchUpdates(): Promise<UpdatesListResponse> {
	const res = await api.get('/updates');
	if (!res.ok) throw new Error(`Failed to fetch updates: ${res.status}`);
	return res.json();
}

/** Trigger a check for all updates */
export async function checkUpdates(): Promise<void> {
	const res = await api.post('/updates/check');
	if (!res.ok) {
		const text = await res.text();
		throw new Error(`Failed to trigger update check: ${res.status} ${text}`);
	}
}

/** Update a backend */
export async function updateBackend(name: string): Promise<{ job_id: string }> {
	const res = await api.post(`/backends/${name}/update`);
	if (!res.ok) {
		const text = await res.text();
		throw new Error(`Failed to update backend: ${res.status} ${text}`);
	}
	return res.json();
}

/** Apply model update with selected quants */
export async function applyModelUpdate(
	modelId: string,
	quants: string[]
): Promise<void> {
	const res = await api.post(`/updates/apply/model/${modelId}`, { quants });
	if (!res.ok) {
		const text = await res.text();
		throw new Error(`Failed to apply model update: ${res.status} ${text}`);
	}
}

/** Refresh a specific backend check */
export async function checkBackend(name: string): Promise<void> {
	const res = await api.post(`/updates/check/backend/${name}`);
	if (!res.ok) {
		const text = await res.text();
		console.warn(`Backend check failed for ${name}: ${res.status} ${text}`);
	}
}
