export interface Toast {
	id: string;
	severity: 'info' | 'success' | 'warning' | 'error';
	title: string;
	message: string;
	durationMs: number;
}
