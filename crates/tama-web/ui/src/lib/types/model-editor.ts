/** What kind of file a quant entry represents. */
export type QuantKind = 'model' | 'mmproj';

/** Information about a single quant file. */
export interface QuantInfo {
  file: string;
  kind: QuantKind;
  size_bytes?: number;
  context_length?: number;
  // DB-enriched fields (read-only from backend)
  lfs_oid?: string;
  db_size_bytes?: number;
  last_verified_at?: string;
  verified_ok?: boolean;
  verify_error?: string;
}

/** A sampling field with enabled toggle and value. */
export interface SamplingField {
  enabled: boolean;
  value: string;
}

/** Speculative decoding configuration for the model editor form. */
export interface SpecDecodingForm {
  spec_types: string[];
  n_max?: number;
  n_min?: number;
  draft_ngl?: number;
}

/** Model modality configuration. */
export interface ModelModalities {
  input: string[];
  output: string[];
}

/** Backend option from the server. */
export interface BackendOption {
  name: string;
  variant?: string;
  label: string;
}

/** Full model detail returned from GET /tama/v1/models/:id */
export interface ModelDetail {
  id: number;
  backend: string;
  gpu_variant?: string;
  model?: string;
  quant?: string;
  mmproj?: string;
  args: string[];
  sampling?: Record<string, unknown>;
  enabled: boolean;
  context_length?: number;
  num_parallel?: number;
  port?: number;
  api_name?: string;
  display_name?: string;
  kv_unified: boolean;
  gpu_layers?: number;
  cache_type_k?: string;
  cache_type_v?: string;
  hf_context_length?: number;
  quants: Record<string, QuantInfo>;
  backends: BackendOption[];
  repo_commit_sha?: string;
  repo_pulled_at?: string;
  modalities?: ModelModalities;
  spec_decoding?: Record<string, unknown>;
}

/** Response from GET /tama/v1/models (list endpoint). */
export interface ModelListResponse {
  models: unknown[];
  backends: BackendOption[];
  sampling_templates?: Record<string, Record<string, unknown>>;
}

/** Consolidated form used by the editor — all fields in one object. */
export interface ModelForm {
  id: string;
  backend: string;
  gpu_variant?: string;
  model?: string;
  quant?: string;
  mmproj?: string;
  args: string; // newline-separated
  sampling: Record<string, SamplingField>;
  enabled: boolean;
  context_length?: number;
  num_parallel?: number;
  port?: number;
  api_name?: string;
  display_name?: string;
  kv_unified: boolean;
  gpu_layers?: number;
  cache_type_k?: string;
  cache_type_v?: string;
  hf_context_length?: number;
  quants: Record<string, QuantInfo>;
  modalities?: ModelModalities;
  spec_decoding: SpecDecodingForm;
}

/** DB file record returned from refresh/verify responses. */
export interface FileRecordJson {
  filename: string;
  lfs_oid?: string;
  size_bytes?: number;
  last_verified_at?: string;
  verified_ok?: boolean;
  verify_error?: string;
}

/** Response from POST /tama/v1/models/:id/refresh */
export interface RefreshResponse {
  repo_commit_sha?: string;
  repo_pulled_at?: string;
  files: FileRecordJson[];
}

/** Response from POST /tama/v1/models/:id/verify */
export interface VerifyResponse {
  ok: boolean;
  any_unknown: boolean;
  files: FileRecordJson[];
}

/** Known sampling field keys. */
export const SAMPLING_FIELDS = [
  'temperature',
  'top_k',
  'top_p',
  'min_p',
  'presence_penalty',
  'frequency_penalty',
  'repeat_penalty'
] as const;

export type SamplingFieldKey = (typeof SAMPLING_FIELDS)[number];

/** Spec decoding type options. */
export const SPEC_TYPES = ['draft-mtp', 'ngram-simple'] as const;
export type SpecType = (typeof SPEC_TYPES)[number];

/** Cache type options for K/V cache. */
export const CACHE_TYPES = ['f16', 'q8_0', 'q4_0'] as const;
export type CacheType = (typeof CACHE_TYPES)[number];
