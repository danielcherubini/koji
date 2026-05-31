<script lang="ts">
	import { formatRelativeTime, formatDurationLabel } from '$lib/utils/formatting';

	interface Props {
		data: number[];
		maxValue: number;
		color: string;
		height?: number;
		timestamps?: number[];
		unitLabel?: string;
		yRefs?: number[];
	}

	let {
		data,
		maxValue,
		color,
		height = 60,
		timestamps = [],
		unitLabel = '',
		yRefs = []
	}: Props = $props();

	// Chart area: leave 10px at bottom for time labels
	let chartBottom = $derived(height - 10);

	// Normalized data (handle empty/single point)
	let chartData = $derived<number[]>(data.length === 0 ? [] : data.length === 1 ? [data[0], data[0]] : data);
	let chartTimestamps = $derived<number[]>(
		timestamps.length === 0
			? []
			: timestamps.length === 1
				? [timestamps[0], timestamps[0]]
				: timestamps
	);

	// Hover state
	let mouseX = $state(-1);
	let hoverIndex = $state(-1);
	let showTooltip = $state(false);

	// Build SVG path for area chart
	function buildPath(values: number[]): string {
		if (values.length === 0) return '';
		const len = values.length;
		const points = values.map((v, i) => {
			const x = (i / (len - 1)) * 100;
			const y = chartBottom - (v / maxValue) * chartBottom;
			return { x, y };
		});

		// Area fill path
		let d = `M ${points[0].x} ${points[0].y}`;
		for (let i = 1; i < points.length; i++) {
			d += ` L ${points[i].x} ${points[i].y}`;
		}
		d += ` L 100 ${chartBottom} L 0 ${chartBottom} Z`;
		return d;
	}

	function buildLinePath(values: number[]): string {
		if (values.length === 0) return '';
		const len = values.length;
		const points = values.map((v, i) => {
			const x = (i / (len - 1)) * 100;
			const y = chartBottom - (v / maxValue) * chartBottom;
			return { x, y };
		});

		let d = `M ${points[0].x} ${points[0].y}`;
		for (let i = 1; i < points.length; i++) {
			d += ` L ${points[i].x} ${points[i].y}`;
		}
		return d;
	}

	function handleMouseMove(e: MouseEvent) {
		const rect = (e.currentTarget as SVGElement).getBoundingClientRect();
		const relX = ((e.clientX - rect.left) / rect.width) * 100;
		mouseX = relX;

		if (chartData.length === 0) {
			hoverIndex = -1;
			showTooltip = false;
			return;
		}

		const len = chartData.length;
		const idx = Math.round((relX / 100) * (len - 1));
		hoverIndex = Math.max(0, Math.min(len - 1, idx));
		showTooltip = true;
	}

	function handleMouseLeave() {
		showTooltip = false;
		hoverIndex = -1;
	}

	function getHoverPoint(): { x: number; y: number } | null {
		if (hoverIndex < 0 || chartData.length === 0) return null;
		const len = chartData.length;
		const x = (hoverIndex / (len - 1)) * 100;
		const y = chartBottom - (chartData[hoverIndex] / maxValue) * chartBottom;
		return { x, y };
	}

	let hoverPoint = $derived(getHoverPoint());

	// Time axis: total span in seconds
	let timeSpanSecs = $derived(
		chartTimestamps.length >= 2
			? Math.round((chartTimestamps[chartTimestamps.length - 1] - chartTimestamps[0]) / 1000)
			: 0
	);
</script>

<div
	class="sparkline-container"
	style="height: {height}px;"
	onmousemove={handleMouseMove}
	onmouseleave={handleMouseLeave}
	role="img"
	aria-label="Sparkline chart"
>
	<svg
		viewBox={`0 0 100 ${height}`}
		preserveAspectRatio="none"
		class="sparkline-svg"
	>
		<!-- Y-axis reference lines -->
		{#each yRefs as ref}
			{#if ref >= 0 && maxValue > 0}
				<line
					x1="0"
					y1={chartBottom - (ref / maxValue) * chartBottom}
					x2="100"
					y2={chartBottom - (ref / maxValue) * chartBottom}
					stroke="var(--color-border-default)"
					stroke-width="0.5"
					stroke-dasharray="2,2"
				/>
			{/if}
		{/each}

		{#if chartData.length > 0}
			<!-- Area fill -->
			<path d={buildPath(chartData)} fill={color} opacity="0.2" />

			<!-- Line stroke -->
			<path
				d={buildLinePath(chartData)}
				fill="none"
				stroke={color}
				stroke-width="1.5"
				vector-effect="non-scaling-stroke"
			/>
		{/if}

		<!-- Hover overlay -->
		{#if showTooltip && hoverPoint}
			<!-- Vertical dashed line -->
			<line
				x1={hoverPoint.x}
				y1="0"
				x2={hoverPoint.x}
				y2={chartBottom}
				stroke="var(--color-text-secondary)"
				stroke-width="0.5"
				stroke-dasharray="2,2"
				vector-effect="non-scaling-stroke"
			/>

			<!-- Dot on data point -->
			<circle
				cx={hoverPoint.x}
				cy={hoverPoint.y}
				r="2"
				fill={color}
				stroke="var(--color-bg-primary)"
				stroke-width="0.5"
				vector-effect="non-scaling-stroke"
			/>
		{/if}

		<!-- Time axis labels -->
		{#if chartTimestamps.length >= 2}
			<text
				x="0"
				y={height - 1}
				font-size="4"
				fill="var(--color-text-muted)"
				text-anchor="start"
				dominant-baseline="auto"
			>
				{formatDurationLabel(timeSpanSecs)}
			</text>
			<text
				x="100"
				y={height - 1}
				font-size="4"
				fill="var(--color-text-muted)"
				text-anchor="end"
				dominant-baseline="auto"
			>
				now
			</text>
		{/if}
	</svg>

	<!-- Tooltip -->
	{#if showTooltip && hoverIndex >= 0 && hoverIndex < chartData.length}
		<div class="sparkline-tooltip" style="left: {hoverPoint ? (hoverPoint.x / 100) * 100 : 50}%; ">
			<span class="sparkline-tooltip-value">
				{chartData[hoverIndex].toFixed(1)}
				{unitLabel}
			</span>
			{#if chartTimestamps.length > 0}
				<span class="sparkline-tooltip-time">
					{formatRelativeTime(chartTimestamps[hoverIndex])}
				</span>
			{/if}
		</div>
	{/if}
</div>

<style>
	.sparkline-container {
		position: relative;
		width: 100%;
	}

	.sparkline-svg {
		width: 100%;
		height: 100%;
		display: block;
	}

	.sparkline-tooltip {
		position: absolute;
		top: 2px;
		transform: translateX(-50%);
		background: var(--color-bg-tertiary);
		border: 1px solid var(--color-border-default);
		border-radius: 4px;
		padding: 2px 6px;
		font-size: 11px;
		white-space: nowrap;
		pointer-events: none;
		z-index: 10;
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 1px;
	}

	.sparkline-tooltip-value {
		color: var(--color-text-primary);
		font-weight: 500;
	}

	.sparkline-tooltip-time {
		color: var(--color-text-muted);
		font-size: 10px;
	}
</style>
