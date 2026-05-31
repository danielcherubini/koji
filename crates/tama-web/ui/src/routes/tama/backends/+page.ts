import { listBackends, systemCapabilities } from '$lib/api/backends';

export async function load() {
	const [backendsData, capabilitiesData] = await Promise.all([
		listBackends().catch(
			() => ({ backends: [], custom: [], available: [], active_job: null })
		),
		systemCapabilities().catch(
			() => ({
				os: 'unknown',
				arch: 'unknown',
				git_available: false,
				cmake_available: false,
				compiler_available: false,
				supported_cuda_versions: []
			})
		)
	]);

	return {
		backends: backendsData.backends || [],
		custom: backendsData.custom || [],
		available: backendsData.available || [],
		activeJob: backendsData.active_job || null,
		capabilities: capabilitiesData
	};
}
