import { api } from '$lib/api/client';
import type { Config } from '$lib/types/config';

/** Fetch the full structured config from the server. */
export async function getConfig(): Promise<Config> {
	const res = await api.get('/config/structured');
	if (!res.ok) {
		const text = await res.text();
		throw new Error(`Failed to fetch config: ${res.status} ${text}`);
	}
	return res.json();
}

/** Save the full structured config to the server. */
export async function saveConfig(data: Config): Promise<void> {
	const res = await api.post('/config/structured', data);
	if (!res.ok) {
		const text = await res.text();
		throw new Error(`Failed to save config: ${res.status} ${text}`);
	}
}
