/** Format a number with locale-aware thousands separators. */
export function formatNumber(n: number): string {
	return n.toLocaleString('en-US');
}

/** Format a unix-ms timestamp as a relative time string (e.g. "5s ago"). */
export function formatRelativeTime(tsUnixMs: number): string {
	if (tsUnixMs === 0) return '';
	const diffMs = Date.now() - tsUnixMs;
	if (diffMs < 0) return '';
	const secs = Math.floor(diffMs / 1000);
	if (secs < 60) return `${secs}s ago`;
	const mins = Math.floor(secs / 60);
	const remainSecs = secs % 60;
	if (secs < 3600) return remainSecs === 0 ? `${mins}m ago` : `${mins}m ${remainSecs}s ago`;
	return `${Math.floor(secs / 3600)}h ago`;
}

/** Format a duration in seconds as a negative label (e.g. "-5m"). */
export function formatDurationLabel(secs: number): string {
	if (secs < 60) return `-${secs}s`;
	if (secs < 3600) return `-${Math.floor(secs / 60)}m`;
	return `-${Math.floor(secs / 3600)}h`;
}

/** Format MiB as a human-readable string. */
export function formatMib(mib: number): string {
	if (mib >= 1024) {
		return `${(mib / 1024).toFixed(1)} GB`;
	}
	return `${mib} MiB`;
}
