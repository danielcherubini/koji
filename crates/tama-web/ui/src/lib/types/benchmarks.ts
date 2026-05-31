// ── Benchmark run request (Standard / llama-bench) ───────────────────

export interface BenchmarkConfig {
  model_id: string;
  quant?: string;
  backend_name?: string;
  pp_sizes: number[];
  tg_sizes: number[];
  runs: number;
  warmup: number;
  threads?: number[];
  ngl_range?: string;
  ctx_override?: number;
  batch_sizes: number[];
  ubatch_sizes: number[];
  kv_cache_type?: string;
  depth: number[];
  flash_attn?: boolean;
  benchmark_type?: string;
}

// ── Spec decoding benchmark request ───────────────────────────────────

export type SpecType = 'ngram' | 'ngram-mod' | 'incontext' | 'mtp';

export interface SpecBenchmarkConfig {
  model_id: string;
  quant?: string;
  backend_name?: string;
  gpu_variant?: string;
  spec_types: SpecType[];
  draft_max_values: number[];
  ngram_n_values: number[];
  ngram_m_values: number[];
  ngram_min_values: number[];
  ngram_max_values: number[];
  ngram_min_hits: number;
  gen_tokens: number;
  runs: number;
  ngl?: number;
  flash_attn: boolean;
  benchmark_type?: string;
}

// ── MTP benchmark request ────────────────────────────────────────────

export interface MtpBenchmarkConfig {
  model_id: string;
  quant?: string;
  backend_name?: string;
  gpu_variant?: string;
  draft_max_values: number[];
  ngl?: number;
  draft_ngl?: number;
  flash_attn: boolean;
  context_size?: number;
  benchmark_type?: string;
}

// ── Benchmark run response ───────────────────────────────────────────

export interface BenchmarkRunResponse {
  job_id: string;
}

// ── History entry (matches API response) ─────────────────────────────

export interface HistoryEntry {
  id: number;
  created_at: number;
  model_id: string;
  display_name?: string;
  quant?: string;
  backend: string;
  engine?: string;
  benchmark_type?: string;
  pp_sizes: number[];
  tg_sizes: number[];
  runs: number;
  results_count: number;
  status: string;
  results: any;
}

// ── Benchmark presets (mirror of Rust BenchmarkPreset::all()) ────────

export interface BenchmarkPreset {
  label: string;
  description: string;
  pp_sizes: number[];
  tg_sizes: number[];
  runs: number;
  threads?: number[];
  ngl_range?: string;
  batch_sizes: number[];
  ubatch_sizes: number[];
  kv_cache_type?: string;
  depth: number[];
  flash_attn?: boolean;
}

/** Standard benchmark presets for the tuning methodology. */
export const BENCHMARK_PRESETS: BenchmarkPreset[] = [
  {
    label: '1. Baseline',
    description: 'Known-good flags. Record PP and TG as the reference point.',
    pp_sizes: [2048],
    tg_sizes: [128],
    runs: 3,
    ngl_range: '99',
    batch_sizes: [],
    ubatch_sizes: [],
    depth: [],
    flash_attn: true,
  },
  {
    label: '2. Batch sweep',
    description: 'Sweep -ub to find the PP knee. Pick the smallest -ub at the plateau.',
    pp_sizes: [2048],
    tg_sizes: [128],
    runs: 3,
    ngl_range: '99',
    batch_sizes: [4096],
    ubatch_sizes: [512, 1024, 2048, 4096],
    depth: [],
    flash_attn: true,
  },
  {
    label: '3a. KV quant (q8_0)',
    description: 'KV quant baseline at depth. Rerun with q4_0 next to compare.',
    pp_sizes: [0],
    tg_sizes: [128],
    runs: 3,
    ngl_range: '99',
    batch_sizes: [4096],
    ubatch_sizes: [2048],
    kv_cache_type: 'q8_0',
    depth: [0, 65536, 131072],
    flash_attn: true,
  },
  {
    label: '3b. KV quant (q4_0)',
    description: 'Half-size KV cache. Usually ties q8_0 at d=0; pulls ahead at 128k+.',
    pp_sizes: [0],
    tg_sizes: [128],
    runs: 3,
    ngl_range: '99',
    batch_sizes: [4096],
    ubatch_sizes: [2048],
    kv_cache_type: 'q4_0',
    depth: [0, 65536, 131072],
    flash_attn: true,
  },
  {
    label: '4. Depth validation',
    description: 'Lock winning KV config; run at your real target depth. Edit -d.',
    pp_sizes: [0],
    tg_sizes: [128],
    runs: 3,
    ngl_range: '99',
    batch_sizes: [4096],
    ubatch_sizes: [2048],
    kv_cache_type: 'q8_0',
    depth: [131072],
    flash_attn: true,
  },
];

// ── Spec decoding presets (mirror of Rust SPEC_BENCH_PRESETS) ────────

export interface SpecPreset {
  label: string;
  draft_max_values: number[];
  ngram_n_values: string; // comma-separated
  ngram_m_values: string;
  ngram_max_values: string;
}

export const SPEC_PRESETS: SpecPreset[] = [
  {
    label: 'Spec Scan',
    draft_max_values: [256],
    ngram_n_values: '16',
    ngram_m_values: '12',
    ngram_max_values: '48',
  },
  {
    label: 'Spec Sweep',
    draft_max_values: [8, 16, 32, 48, 64],
    ngram_n_values: '8,16,32,48,64',
    ngram_m_values: '12,16,24',
    ngram_max_values: '32,48',
  },
];

// ── MTP defaults ─────────────────────────────────────────────────────

export const DEFAULT_DRAFT_MAX_VALUES: number[] = [0, 1, 2, 3, 4, 5, 6, 7, 8];
export const DEFAULT_NGL: number = 99;
export const DEFAULT_DRAFT_NGL: number = 99;
export const DEFAULT_CONTEXT_SIZE: number = 32768;

// ── Helpers ───────────────────────────────────────────────────────────

/** Parse a comma-separated string into a number array. */
export function parseNumList(s: string): number[] {
  if (!s.trim()) return [];
  return s
    .split(',')
    .map((n) => n.trim())
    .filter(Boolean)
    .map(Number)
    .filter((n) => !isNaN(n));
}

/** Format a number array as a comma-separated string. */
export function formatNumList(nums: number[]): string {
  return nums.join(',');
}

/** Format a timestamp (unix seconds) as a readable string. */
export function formatBenchmarkTime(ts: number): string {
  if (!ts) return '';
  return new Date(ts * 1000).toLocaleString();
}

/** Get the display name for a history entry. */
export function getHistoryDisplayName(entry: HistoryEntry): string {
  return entry.display_name || entry.model_id || `#${entry.id}`;
}
