export interface DownloadItem {
  job_id: string;
  repo_id: string;
  filename: string;
  display_name: string | null;
  status: string;
  bytes_downloaded: number;
  total_bytes: number | null;
  error_message: string | null;
  started_at: string | null;
  completed_at: string | null;
  queued_at: string;
  kind: string;
}

export interface ActiveDownloadsResponse {
  items: DownloadItem[];
}

export interface HistoryDownloadsResponse {
  items: DownloadItem[];
  total: number;
}
