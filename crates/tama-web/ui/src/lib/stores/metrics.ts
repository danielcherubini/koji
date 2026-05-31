import { writable } from 'svelte/store';
import type { MetricSample } from '$lib/types/metrics';

export const metricsHistory = writable<MetricSample[]>([]);
export const metricsError = writable(false);

let eventSource: EventSource | null = null;

export function connectMetrics(): void {
	if (eventSource) return;
	eventSource = new EventSource('/tama/v1/system/metrics/stream');
	eventSource.addEventListener('snapshot', (e: MessageEvent) => {
		try {
			const data = JSON.parse(e.data) as MetricSample[];
			metricsHistory.set(data);
			metricsError.set(false);
		} catch {
			metricsError.set(true);
		}
	});
	eventSource.onerror = () => metricsError.set(true);
}

export function disconnectMetrics(): void {
	eventSource?.close();
	eventSource = null;
}
