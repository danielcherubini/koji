export interface SourceLogs {
	name: string;
	lines: string[];
}

export interface AllLogsResponse {
	sources: SourceLogs[];
}
