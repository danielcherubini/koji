import { writable, type Writable } from 'svelte/store';
import { browser } from '$app/environment';
import type { DownloadItem } from '$lib/types/downloads';

export interface DownloadEvent {
	event: string;
	job_id: string;
	filename?: string;
	repo_id?: string;
	bytes_downloaded?: number;
	total_bytes?: number;
	size_bytes?: number;
	duration_ms?: number;
	error?: string;
}

export interface ActiveDownload {
	job_id: string;
	filename: string;
	status: string;
	bytes_downloaded: number;
	total_bytes?: number;
}

export const activeDownloads: Writable<ActiveDownload[]> = writable([]);
export const downloadHistory: Writable<DownloadItem[]> = writable([]);

let eventSource: EventSource | null = null;

async function fetchActiveDownloads(): Promise<void> {
	try {
		const resp = await fetch('/tama/v1/downloads/active', { credentials: 'same-origin' });
		if (resp.ok) activeDownloads.set(await resp.json());
	} catch {
		/* silently fail */
	}
}

async function fetchDownloadHistory(): Promise<void> {
	try {
		const resp = await fetch('/tama/v1/downloads/history', { credentials: 'same-origin' });
		if (resp.ok) downloadHistory.set(await resp.json());
	} catch {
		/* silently fail */
	}
}

function handleDownloadEvent(event: DownloadEvent): void {
	switch (event.event) {
		case 'Started':
		case 'Progress':
		case 'Verifying':
			activeDownloads.update((downloads) => {
				const idx = downloads.findIndex((d) => d.job_id === event.job_id);
				const download: ActiveDownload = {
					job_id: event.job_id,
					filename: event.filename || downloads[idx]?.filename || '',
					status: event.event.toLowerCase(),
					bytes_downloaded: event.bytes_downloaded || 0,
					total_bytes: event.total_bytes ?? downloads[idx]?.total_bytes
				};
				if (idx >= 0) downloads[idx] = download;
				else downloads.push(download);
				return downloads;
			});
			break;
		case 'Completed':
		case 'Failed':
		case 'Cancelled':
			activeDownloads.update((d) => d.filter((d) => d.job_id !== event.job_id));
			fetchDownloadHistory();
			break;
		case 'Queued':
			fetchActiveDownloads();
			break;
	}
}

export function startDownloadEvents(): void {
	if (!browser || eventSource) return;
	eventSource = new EventSource('/tama/v1/downloads/events');
	eventSource.onmessage = (e: MessageEvent) => {
		try {
			handleDownloadEvent(JSON.parse(e.data));
		} catch {
			/* ignore */
		}
	};
}

export function stopDownloadEvents(): void {
	eventSource?.close();
	eventSource = null;
}
