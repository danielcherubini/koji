import { api } from '$lib/api/client';
import type { AllLogsResponse } from '$lib/types/logs';

/** Fetch all logs from all sources */
export async function fetchLogs(): Promise<AllLogsResponse> {
	const res = await api.get('/logs');
	if (!res.ok) {
		if (res.status >= 400 && res.status < 500) {
			// logs_dir may not be configured — return empty
			return { sources: [] };
		}
		throw new Error(`Failed to fetch logs: ${res.status}`);
	}
	return res.json();
}
