import { listModels } from '$lib/api/models';

export async function load() {
	const data = await listModels().catch(() => ({ models: [] }));
	return { models: data.models || [] };
}
