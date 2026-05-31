import { api } from '$lib/api/client';
import type {
  BackendListResponse,
  BackendCardDto,
  CapabilitiesDto,
  InstallRequest,
  InstallResponse,
  ActivateResponse
} from '$lib/types/backends';

/** List all backends (installed + available + custom). */
export async function listBackends(): Promise<BackendListResponse> {
  const res = await api.get('/backends');
  if (!res.ok) throw new Error(`Failed to fetch backends: ${res.status}`);
  return res.json();
}

/** Install a backend. Returns a job_id for tracking progress. */
export async function installBackend(data: InstallRequest): Promise<InstallResponse> {
  const res = await api.post('/backends/install', data);
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`Failed to install backend: ${res.status} ${text}`);
  }
  return res.json();
}

/** Update an installed backend to the latest version. Returns a job_id. */
export async function updateBackend(
  name: string,
  gpuVariant?: string
): Promise<{ job_id: string }> {
  const params = gpuVariant ? `?gpu_variant=${encodeURIComponent(gpuVariant)}` : '';
  const res = await api.post(`/backends/${encodeURIComponent(name)}/update${params}`);
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`Failed to update backend: ${res.status} ${text}`);
  }
  return res.json();
}

/** Remove (uninstall) a backend. */
export async function removeBackend(
  name: string,
  gpuVariant?: string
): Promise<{ removed: boolean }> {
  const params = gpuVariant ? `?gpu_variant=${encodeURIComponent(gpuVariant)}` : '';
  const res = await api.delete(`/backends/${encodeURIComponent(name)}${params}`);
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`Failed to remove backend: ${res.status} ${text}`);
  }
  return res.json();
}

/** Activate a specific version of an installed backend. */
export async function activateVersion(
  name: string,
  version: string,
  gpuVariant?: string
): Promise<ActivateResponse> {
  const params = gpuVariant ? `?gpu_variant=${encodeURIComponent(gpuVariant)}` : '';
  const res = await api.post(
    `/backends/${encodeURIComponent(name)}/activate${params}`,
    { version }
  );
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`Failed to activate version: ${res.status} ${text}`);
  }
  return res.json();
}

/** Update the default arguments for a backend. */
export async function updateDefaultArgs(
  name: string,
  defaultArgs: string[],
  gpuVariant?: string
): Promise<void> {
  const params = gpuVariant ? `?gpu_variant=${encodeURIComponent(gpuVariant)}` : '';
  const res = await api.post(
    `/backends/${encodeURIComponent(name)}/default-args${params}`,
    { default_args: defaultArgs }
  );
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`Failed to update default args: ${res.status} ${text}`);
  }
}

/** Get system capabilities (OS, arch, CUDA, build tools). */
export async function systemCapabilities(): Promise<CapabilitiesDto> {
  const res = await api.get('/system/capabilities');
  if (!res.ok) throw new Error(`Failed to fetch capabilities: ${res.status}`);
  return res.json();
}

/** Check for updates on all backends, then return the refreshed list. */
export async function checkAllUpdates(): Promise<BackendListResponse> {
  const res = await api.post('/backends/check-updates');
  if (!res.ok) throw new Error(`Failed to check updates: ${res.status}`);
  return res.json();
}

/** Check for updates on a single backend. */
export async function checkBackendUpdates(
  name: string,
  gpuVariant?: string
): Promise<BackendCardDto> {
  const params = gpuVariant ? `?gpu_variant=${encodeURIComponent(gpuVariant)}` : '';
  const res = await api.post(`/backends/${encodeURIComponent(name)}/check-updates${params}`);
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`Failed to check updates: ${res.status} ${text}`);
  }
  return res.json();
}
