export interface Alias {
	id: number;
	name: string;
	model_id: number;
	model_name: string;
	description: string | null;
	enabled: boolean;
	created_at: string;
	updated_at: string;
}

export interface ModelOption {
	id: number;
	label: string;
}

export interface CreateAliasForm {
	name: string;
	model_id: number;
	description: string;
}

export interface UpdateAliasForm {
	name?: string;
	model_id?: number;
	description?: string | null;
	enabled?: boolean;
}
