import { api } from '$lib/api/client';

/** Metadata about a file available from a HuggingFace repo. */
export interface HfFileInfo {
  filename: string;
  size: number;
  kind: 'model' | 'mmproj';
  quant?: string;
}

/** Metadata returned from fetching a HuggingFace repo. */
export interface HfMetadata {
  repo_id: string;
  model_id: string;
  quants: HfFileInfo[];
  mmprojs: HfFileInfo[];
  context_length?: number;
}

/** Request to start a pull from HuggingFace. */
export interface PullRequest {
  repo_id: string;
  files: string[];
  model_id?: string;
  context_length?: number;
  kv_unified?: boolean;
  cache_type_k?: string;
  cache_type_v?: string;
}

/** Completed quant after a pull. */
export interface CompletedQuant {
  filename: string;
  quant?: string;
  size_bytes?: number;
}

/** Download progress for a single file. */
export interface DownloadProgress {
  filename: string;
  bytes_downloaded: number;
  total_bytes: number | null;
  status: string;
  error?: string;
}

/** Fetch HuggingFace metadata for a repo. */
export async function fetchHfMetadata(repoId: string): Promise<HfMetadata | null> {
  const encodedId = encodeURIComponent(repoId);
  const res = await api.get(`/pull/hf-metadata/${encodedId}`);
  if (!res.ok) {
    if (res.status === 404) return null;
    const text = await res.text();
    throw new Error(`Failed to fetch HF metadata: ${res.status} ${text}`);
  }
  return res.json();
}

/** Create a new model entry. */
export async function createModel(data: {
  id: string;
  backend: string;
  model?: string;
  quants?: Record<string, unknown>;
  context_length?: number;
  kv_unified?: boolean;
  cache_type_k?: string;
  cache_type_v?: string;
}): Promise<void> {
  const res = await api.post('/models', data);
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`Failed to create model: ${res.status} ${text}`);
  }
}

/** Start pulling files from HuggingFace. */
export async function startPull(request: PullRequest): Promise<{ job_id: string }> {
  const res = await api.post('/pull/start', request);
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`Failed to start pull: ${res.status} ${text}`);
  }
  return res.json();
}

/** Get pull progress for a job. */
export async function getPullProgress(jobId: string): Promise<DownloadProgress[]> {
  const encodedId = encodeURIComponent(jobId);
  const res = await api.get(`/pull/progress/${encodedId}`);
  if (!res.ok) {
    throw new Error(`Failed to get pull progress: ${res.status}`);
  }
  return res.json();
}
