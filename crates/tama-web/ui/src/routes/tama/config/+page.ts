import { getConfig } from '$lib/api/config';
import type { PageLoad } from './$types';

export const load: PageLoad = async () => {
	try {
		const config = await getConfig();
		return { config };
	} catch (e) {
		console.error('Failed to load config:', e);
		return { config: null, error: (e as Error).message };
	}
};
