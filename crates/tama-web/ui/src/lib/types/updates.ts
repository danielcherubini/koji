export interface UpdateCheckDto {
	item_type: string;
	item_id: string;
	variant?: string | null;
	repo_id?: string | null;
	display_name?: string | null;
	current_version?: string | null;
	latest_version?: string | null;
	update_available: boolean;
	status: string;
	error_message?: string | null;
	checked_at: number;
	details_json?: any;
}

export interface UpdatesListResponse {
	backends: UpdateCheckDto[];
	models: UpdateCheckDto[];
}
