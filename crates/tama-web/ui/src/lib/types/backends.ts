// ── Backend card DTOs (mirror of tama-web API types) ──────────────────

export interface GpuTypeDto {
  kind: 'cuda' | 'vulkan' | 'metal' | 'rocm' | 'cpu_only' | 'custom';
  version?: string;
}

export interface BackendSourceDto {
  kind: 'prebuilt' | 'source_code';
  version: string;
  git_url?: string;
  commit?: string;
}

export interface BackendInfoDto {
  name: string;
  version: string;
  path: string;
  installed_at: number;
  gpu_variant: string;
  gpu_type?: GpuTypeDto;
  source?: BackendSourceDto;
}

export interface BackendVersionDto {
  name: string;
  version: string;
  path: string;
  installed_at: number;
  gpu_variant: string;
  gpu_type?: GpuTypeDto;
  source?: BackendSourceDto;
  is_active: boolean;
}

export interface UpdateStatusDto {
  checked: boolean;
  latest_version?: string;
  update_available?: boolean;
}

export interface BackendCardDto {
  type: string;
  display_name: string;
  installed: boolean;
  gpu_variant: string;
  info?: BackendInfoDto;
  versions: BackendVersionDto[];
  update: UpdateStatusDto;
  release_notes_url?: string;
  default_args: string[];
  is_active: boolean;
}

export interface ActiveJobDto {
  id: string;
  kind: string;
  backend_type: string;
}

export interface BackendListResponse {
  active_job?: ActiveJobDto;
  backends: BackendCardDto[];
  custom: BackendCardDto[];
  available: string[];
}

// ── System capabilities ───────────────────────────────────────────────

export interface CapabilitiesDto {
  os: string;
  arch: string;
  git_available: boolean;
  cmake_available: boolean;
  compiler_available: boolean;
  detected_cuda_version?: string;
  supported_cuda_versions: string[];
}

// ── Install request ───────────────────────────────────────────────────

export interface InstallRequest {
  backend_type: string;
  version?: string;
  gpu_type: GpuTypeDto;
  build_from_source: boolean;
  force: boolean;
}

// ── Install response ──────────────────────────────────────────────────

export interface InstallResponse {
  job_id: string;
  kind: string;
  backend_type: string;
  notices?: string[];
}

// ── Activate response ─────────────────────────────────────────────────

export interface ActivateResponse {
  version: string;
  is_active: boolean;
}

// ── Helpers ───────────────────────────────────────────────────────────

/** Format a GPU type DTO into a human-readable label. */
export function gpuTypeLabel(gpu: GpuTypeDto): string {
  switch (gpu.kind) {
    case 'cuda':
      return `CUDA ${gpu.version || '?'}`;
    case 'vulkan':
      return 'Vulkan';
    case 'metal':
      return 'Metal';
    case 'rocm':
      return `ROCm ${gpu.version || '?'}`;
    case 'cpu_only':
      return 'CPU';
    case 'custom':
      return 'Custom';
    default:
      return gpu.kind;
  }
}

/** Format a gpu_variant string for display (e.g. "cuda_12" → "CUDA 12"). */
export function formatGpuVariant(variant: string): string {
  if (!variant) return '';
  return variant.replace(/_/g, ' ').toUpperCase();
}

/** Get the status badge class for a backend card. */
export function getBackendStatusBadge(backend: BackendCardDto): string {
  if (!backend.installed) return 'badge-info';
  if (backend.is_active) return 'badge-success';
  return '';
}

/** Get the status label for a backend card. */
export function getBackendStatusLabel(backend: BackendCardDto): string {
  if (!backend.installed) return 'Not installed';
  if (backend.is_active) return 'Active';
  return 'Installed';
}
