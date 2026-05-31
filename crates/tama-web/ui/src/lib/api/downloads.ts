import { api } from './client';
import type { ActiveDownloadsResponse, HistoryDownloadsResponse } from '$lib/types/downloads';

export async function getActiveDownloads(): Promise<ActiveDownloadsResponse> {
  const resp = await api.get('/downloads/active');
  return resp.json();
}

export async function getDownloadHistory(limit = 50, offset = 0): Promise<HistoryDownloadsResponse> {
  const resp = await api.get(`/downloads/history?limit=${limit}&offset=${offset}`);
  return resp.json();
}

export async function cancelDownload(jobId: string): Promise<void> {
  await api.post(`/downloads/${jobId}/cancel`);
}
