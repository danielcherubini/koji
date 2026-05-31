import { api } from '$lib/api/client';
import type {
  ModelDetail,
  ModelForm,
  ModelListResponse,
  RefreshResponse,
  VerifyResponse,
  SamplingField,
  SpecDecodingForm,
  ModelModalities
} from '$lib/types/model-editor';
import { SAMPLING_FIELDS } from '$lib/types/model-editor';

/** Fetch a model by ID. If id === 'new', fetch the models list for defaults. */
export async function fetchModel(id: string): Promise<ModelDetail | null> {
  if (id === 'new') {
    const list = await fetchModelList();
    if (!list) return null;
    const backends = list.backends ?? [];
    return {
      id: 0,
      backend: backends[0]?.name ?? '',
      gpu_variant: undefined,
      model: undefined,
      quant: undefined,
      mmproj: undefined,
      args: [],
      sampling: undefined,
      enabled: true,
      context_length: undefined,
      num_parallel: 0,
      port: undefined,
      api_name: undefined,
      display_name: undefined,
      kv_unified: true,
      gpu_layers: undefined,
      cache_type_k: undefined,
      cache_type_v: undefined,
      hf_context_length: undefined,
      quants: {},
      backends,
      repo_commit_sha: undefined,
      repo_pulled_at: undefined,
      modalities: undefined,
      spec_decoding: undefined
    };
  }

  const encodedId = encodeURIComponent(id);
  const res = await api.get(`/models/${encodedId}`);
  if (!res.ok) {
    if (res.status === 404) return null;
    throw new Error(`Failed to fetch model: ${res.status}`);
  }
  return res.json();
}

/** Fetch the models list (for backends and sampling templates). */
export async function fetchModelList(): Promise<ModelListResponse | null> {
  const res = await api.get('/models');
  if (!res.ok) {
    throw new Error(`Failed to fetch model list: ${res.status}`);
  }
  return res.json();
}

/** Fetch sampling templates from the models list endpoint. */
export async function fetchSamplingTemplates(): Promise<Record<string, Record<string, unknown>> | null> {
  const list = await fetchModelList();
  return list?.sampling_templates ?? null;
}

/** Convert ModelDetail.sampling JSON to SamplingField map. */
function samplingToFields(sampling: Record<string, unknown> | undefined): Record<string, SamplingField> {
  const fields: Record<string, SamplingField> = {};
  if (!sampling) return fields;

  for (const key of SAMPLING_FIELDS) {
    const val = sampling[key];
    if (val === undefined || val === null) continue;

    if (typeof val === 'number' || typeof val === 'string') {
      fields[key] = { enabled: true, value: String(val) };
    }
  }
  return fields;
}

/** Convert ModelDetail.spec_decoding JSON to SpecDecodingForm. */
function specDecodingToForm(sd: Record<string, unknown> | undefined): SpecDecodingForm {
  if (!sd) {
    return { spec_types: [] };
  }
  return {
    spec_types: Array.isArray(sd.spec_types) ? sd.spec_types : [],
    n_max: typeof sd.n_max === 'number' ? sd.n_max : undefined,
    n_min: typeof sd.n_min === 'number' ? sd.n_min : undefined,
    draft_ngl: typeof sd.draft_ngl === 'number' ? sd.draft_ngl : undefined
  };
}

/** Convert ModelDetail to ModelForm for the editor. */
export function detailToForm(detail: ModelDetail): ModelForm {
  return {
    id: String(detail.id),
    backend: detail.backend,
    gpu_variant: detail.gpu_variant,
    model: detail.model,
    quant: detail.quant,
    mmproj: detail.mmproj,
    args: detail.args.join('\n'),
    sampling: samplingToFields(detail.sampling as Record<string, unknown> | undefined),
    enabled: detail.enabled,
    context_length: detail.context_length,
    num_parallel: detail.num_parallel,
    port: detail.port,
    api_name: detail.api_name,
    display_name: detail.display_name,
    kv_unified: detail.kv_unified,
    gpu_layers: detail.gpu_layers,
    cache_type_k: detail.cache_type_k,
    cache_type_v: detail.cache_type_v,
    hf_context_length: detail.hf_context_length,
    quants: detail.quants,
    modalities: detail.modalities ?? { input: [], output: [] },
    spec_decoding: specDecodingToForm(detail.spec_decoding as Record<string, unknown> | undefined)
  };
}

/** Convert ModelForm sampling fields to sampling JSON for the API. */
function samplingFieldsToJson(sampling: Record<string, SamplingField>): Record<string, unknown> | null {
  const obj: Record<string, unknown> = {};

  for (const [key, field] of Object.entries(sampling)) {
    if (!field.enabled) continue;

    if (key === 'top_k') {
      const val = parseInt(field.value, 10);
      if (!isNaN(val)) obj[key] = val;
    } else {
      const val = parseFloat(field.value);
      if (!isNaN(val)) obj[key] = val;
    }
  }

  return Object.keys(obj).length > 0 ? obj : null;
}

/** Save a model (create or update). */
export async function saveModel(
  args: string[],
  form: ModelForm,
  isNew: boolean
): Promise<void> {
  const sampling = samplingFieldsToJson(form.sampling);

  const body: Record<string, unknown> = {
    id: form.id,
    backend: form.backend,
    gpu_variant: form.gpu_variant,
    model: form.model,
    quant: form.quant,
    mmproj: form.mmproj,
    args,
    sampling,
    enabled: form.enabled,
    context_length: form.context_length,
    num_parallel: form.num_parallel,
    port: form.port,
    api_name: form.api_name,
    display_name: form.display_name,
    kv_unified: form.kv_unified,
    gpu_layers: form.gpu_layers,
    cache_type_k: form.cache_type_k,
    cache_type_v: form.cache_type_v,
    quants: form.quants,
    modalities: form.modalities,
    spec_decoding: form.spec_decoding
  };

  const encodedId = encodeURIComponent(form.id);
  const url = isNew ? '/models' : `/models/${encodedId}`;
  const method = isNew ? 'post' : 'put';

  const res = await api[method](url, body);
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`Failed to save model: ${res.status} ${text}`);
  }
}

/** Rename a model. */
export async function renameModel(oldId: string, newId: string): Promise<void> {
  const encodedId = encodeURIComponent(oldId);
  const res = await api.post(`/models/${encodedId}/rename`, { new_id: newId });
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`Failed to rename model: ${res.status} ${text}`);
  }
}

/** Delete a model. */
export async function deleteModel(id: string): Promise<void> {
  const encodedId = encodeURIComponent(id);
  const res = await api.delete(`/models/${encodedId}`);
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`Failed to delete model: ${res.status} ${text}`);
  }
}

/** Delete a quant from a model. */
export async function deleteQuant(id: string, quantKey: string): Promise<void> {
  const encodedId = encodeURIComponent(id);
  const encodedKey = encodeURIComponent(quantKey);
  const res = await api.delete(`/models/${encodedId}/quants/${encodedKey}`);
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`Failed to delete quant: ${res.status} ${text}`);
  }
}

/** Refresh model metadata. */
export async function refreshModel(id: string): Promise<RefreshResponse> {
  const encodedId = encodeURIComponent(id);
  const res = await api.post(`/models/${encodedId}/refresh`);
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`Failed to refresh model: ${res.status} ${text}`);
  }
  return res.json();
}

/** Verify model files. */
export async function verifyModel(id: string): Promise<VerifyResponse> {
  const encodedId = encodeURIComponent(id);
  const res = await api.post(`/models/${encodedId}/verify`);
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`Failed to verify model: ${res.status} ${text}`);
  }
  return res.json();
}
