import { fetchModel, fetchSamplingTemplates, detailToForm } from '$lib/api/model-editor';
import type { ModelForm, BackendOption } from '$lib/types/model-editor';

export async function load({ params }: { params: { id: string } }) {
  const id = params.id;

  // Fetch model detail and sampling templates in parallel
  const [detail, samplingTemplates] = await Promise.all([
    fetchModel(id).catch(() => null),
    fetchSamplingTemplates().catch(() => null)
  ]);

  let form: ModelForm | null = null;
  let backends: BackendOption[] = [];
  let repoCommitSha: string | undefined;
  let repoPulledAt: string | undefined;

  if (detail) {
    form = detailToForm(detail);
    backends = detail.backends ?? [];
    repoCommitSha = detail.repo_commit_sha;
    repoPulledAt = detail.repo_pulled_at;
  }

  return {
    id,
    form,
    backends,
    samplingTemplates,
    repoCommitSha,
    repoPulledAt,
    isNew: id === 'new'
  };
};
