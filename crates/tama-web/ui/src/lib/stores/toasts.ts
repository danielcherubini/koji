import { writable, type Writable } from 'svelte/store';
import type { Toast } from '$lib/types/common';

const MAX_TOASTS = 5;

export const toasts: Writable<Toast[]> = writable([]);

export function addToast(
	title: string,
	message: string,
	severity: Toast['severity'] = 'info',
	durationMs: number = 5000
): void {
	const id = crypto.randomUUID();
	toasts.update((current) => {
		if (current.length >= MAX_TOASTS) {
			current = current.slice(1);
		}
		current.push({ id, title, message, severity, durationMs });
		return current;
	});

	if (durationMs > 0) {
		setTimeout(() => removeToast(id), durationMs);
	}
}

export function removeToast(id: string): void {
	toasts.update((current) => current.filter((t) => t.id !== id));
}

export function clearToasts(): void {
	toasts.set([]);
}
