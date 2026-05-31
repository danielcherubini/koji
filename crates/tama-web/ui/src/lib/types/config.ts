/** Sampling parameters for LLM inference. */
export interface SamplingParams {
  temperature?: number;
  top_k?: number;
  top_p?: number;
  min_p?: number;
  presence_penalty?: number;
  frequency_penalty?: number;
  repeat_penalty?: number;
}

/** General configuration section. */
export interface General {
  log_level: string;
  models_dir?: string;
  logs_dir?: string;
  hf_token?: string;
}

/** Proxy configuration section. */
export interface ProxyConfig {
  host: string;
  port: number;
  auto_unload: boolean;
  idle_timeout_secs: number;
  startup_timeout_secs: number;
  circuit_breaker_threshold: number;
  circuit_breaker_cooldown_seconds: number;
  metrics_retention_secs: number;
}

/** Supervisor configuration section. */
export interface Supervisor {
  restart_policy: string;
  max_restarts: number;
  restart_delay_ms: number;
  health_check_interval_ms: number;
  health_check_timeout_ms: number;
  health_check_retries: number;
}

/** Full structured config returned by GET /tama/v1/config/structured. */
export interface Config {
  general: General;
  proxy: ProxyConfig;
  supervisor: Supervisor;
  sampling_templates: Record<string, SamplingParams>;
}
