export interface ModelEntry {
  id: number;
  backend: string;
  model: string | null;
  quant: string | null;
  enabled: boolean;
  loaded: boolean;
  state: string;
  api_name: string | null;
  display_name: string | null;
}

export interface ModelsResponse {
  models: ModelEntry[];
}
