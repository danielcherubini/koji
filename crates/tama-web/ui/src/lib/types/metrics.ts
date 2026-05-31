/** VRAM usage in MiB. */
export interface VramInfo {
	used_mib: number;
	total_mib: number;
}

/** Per-model loaded/idle status, embedded in MetricSample.models. */
export interface ModelStatus {
	id: string;
	db_id?: number;
	api_name?: string;
	display_name?: string;
	backend: string;
	/** Deprecated: use state instead. */
	loaded?: boolean;
	/** Current lifecycle state: idle, loading, ready, unloading, failed. */
	state: string;
	quant?: string;
	context_length?: number;
	hf_architecture_type?: string;
	hf_base_model?: string;
	gpu_variant?: string;
	cache_type_k?: string;
	cache_type_v?: string;
	spec_types?: string[];
}

/** A timestamped snapshot of system + proxy metrics. */
export interface MetricSample {
	ts_unix_ms: number;
	cpu_usage_pct: number;
	ram_used_mib: number;
	ram_total_mib: number;
	gpu_utilization_pct?: number;
	vram?: VramInfo;
	models_loaded: number;
	models: ModelStatus[];
	/** Token generation speed (tokens per second). */
	tps?: number;
	/** Prompt processing speed in tokens per second. */
	prompt_tps?: number;
	/** KV-cache hit rate percentage. */
	cache_hit_pct?: number;
	/** Speculative decoding acceptance rate. */
	spec_accept_pct?: number;
	/** True if speculative decoding has been active. */
	spec_decoding_active?: boolean;
	/** Unix ms timestamp of the last inference update. */
	inference_last_updated_ms?: number;
}
