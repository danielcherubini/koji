import { getActiveDownloads, getDownloadHistory } from '$lib/api/downloads';

export async function load() {
  const [active, history] = await Promise.all([
    getActiveDownloads().catch(() => ({ items: [] })),
    getDownloadHistory().catch(() => ({ items: [], total: 0 })),
  ]);
  return {
    activeDownloads: active.items || [],
    historyItems: history.items || [],
    historyTotal: history.total || 0,
  };
}
