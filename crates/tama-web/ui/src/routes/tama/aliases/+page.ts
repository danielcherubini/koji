import { listAliases, fetchModels } from '$lib/api/aliases';

export async function load() {
	const [aliases, models] = await Promise.all([
		listAliases().catch(() => []),
		fetchModels().catch(() => [])
	]);
	return { aliases, models };
}
