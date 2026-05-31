import { api } from '$lib/api/client';
import type {
  BenchmarkConfig,
  SpecBenchmarkConfig,
  MtpBenchmarkConfig,
  BenchmarkRunResponse,
  HistoryEntry
} from '$lib/types/benchmarks';

/** Run a standard llama-bench benchmark. Returns a job_id. */
export async function runBenchmark(config: BenchmarkConfig): Promise<BenchmarkRunResponse> {
  const res = await api.post('/benchmarks/run', {
    model_id: config.model_id,
    quant: config.quant,
    backend_name: config.backend_name,
    pp_sizes: config.pp_sizes,
    tg_sizes: config.tg_sizes,
    runs: config.runs,
    warmup: config.warmup ?? 0,
    threads: config.threads,
    ngl_range: config.ngl_range,
    ctx_override: config.ctx_override,
    batch_sizes: config.batch_sizes,
    ubatch_sizes: config.ubatch_sizes,
    kv_cache_type: config.kv_cache_type,
    depth: config.depth,
    flash_attn: config.flash_attn,
    benchmark_type: config.benchmark_type
  });
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`Failed to run benchmark: ${res.status} ${text}`);
  }
  return res.json();
}

/** Run a spec decoding benchmark. Returns a job_id. */
export async function runSpecBenchmark(
  config: SpecBenchmarkConfig
): Promise<BenchmarkRunResponse> {
  const res = await api.post('/benchmarks/spec/run', {
    model_id: config.model_id,
    quant: config.quant,
    backend_name: config.backend_name,
    gpu_variant: config.gpu_variant,
    spec_types: config.spec_types,
    draft_max_values: config.draft_max_values,
    ngram_n_values: config.ngram_n_values,
    ngram_m_values: config.ngram_m_values,
    ngram_min_values: config.ngram_min_values,
    ngram_max_values: config.ngram_max_values,
    ngram_min_hits: config.ngram_min_hits,
    gen_tokens: config.gen_tokens,
    runs: config.runs,
    ngl: config.ngl,
    flash_attn: config.flash_attn,
    benchmark_type: config.benchmark_type
  });
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`Failed to run spec benchmark: ${res.status} ${text}`);
  }
  return res.json();
}

/** Run an MTP (Multi-Token Prediction) benchmark. Returns a job_id. */
export async function runMtpBenchmark(
  config: MtpBenchmarkConfig
): Promise<BenchmarkRunResponse> {
  const res = await api.post('/benchmarks/mtp/run', {
    model_id: config.model_id,
    quant: config.quant,
    backend_name: config.backend_name,
    gpu_variant: config.gpu_variant,
    draft_max_values: config.draft_max_values,
    ngl: config.ngl,
    draft_ngl: config.draft_ngl,
    flash_attn: config.flash_attn,
    context_size: config.context_size,
    benchmark_type: config.benchmark_type
  });
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`Failed to run MTP benchmark: ${res.status} ${text}`);
  }
  return res.json();
}

/** Get benchmark history entries. */
export async function listBenchmarkHistory(): Promise<HistoryEntry[]> {
  const res = await api.get('/benchmarks/history');
  if (!res.ok) throw new Error(`Failed to fetch benchmark history: ${res.status}`);
  return res.json();
}

/** Delete a benchmark history entry by ID. */
export async function deleteBenchmark(id: number): Promise<{ ok: boolean }> {
  const res = await api.delete(`/benchmarks/${id}`);
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`Failed to delete benchmark: ${res.status} ${text}`);
  }
  return res.json();
}

/** Get the current result for a benchmark job. */
export async function getBenchmarkResult(jobId: string): Promise<{
  job_id: string;
  status: string;
  error?: string;
  log_lines: string[];
  benchmark_results?: string;
}> {
  const res = await api.get(`/benchmarks/${jobId}`);
  if (!res.ok) throw new Error(`Failed to fetch benchmark result: ${res.status}`);
  return res.json();
}
