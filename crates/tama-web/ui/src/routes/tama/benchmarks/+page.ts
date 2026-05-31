import { listModels } from '$lib/api/models';
import { listBenchmarkHistory } from '$lib/api/benchmarks';

export async function load() {
	const [modelsData, historyData] = await Promise.all([
		listModels().catch(() => ({ models: [] })),
		listBenchmarkHistory().catch(() => [])
	]);

	return {
		models: modelsData.models || [],
		history: historyData || []
	};
}
