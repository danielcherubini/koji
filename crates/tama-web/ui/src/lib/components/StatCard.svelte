<script lang="ts">
	import type { Snippet } from 'svelte';

	interface Props {
		title: string;
		value: string;
		secondary?: string;
		sparkline?: Snippet;
	}

	let { title, value, secondary, sparkline }: Props = $props();
</script>

<div class="card stat-card">
	<div class="stat-card-header">
		<span class="stat-card-title">{title}</span>
	</div>
	<div class="stat-card-body">
		<span class="stat-card-value">{value}</span>
		{#if secondary}
			<span class="stat-card-secondary">{secondary}</span>
		{/if}
	</div>
	{#if sparkline}
		<div class="stat-card-sparkline">
			{@render sparkline()}
		</div>
	{/if}
</div>

<style>
	.stat-card {
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.stat-card-header {
		display: flex;
		align-items: center;
		justify-content: space-between;
	}

	.stat-card-title {
		font-size: 12px;
		color: var(--color-text-secondary);
		text-transform: uppercase;
		letter-spacing: 0.05em;
	}

	.stat-card-body {
		display: flex;
		flex-direction: column;
		gap: 2px;
	}

	.stat-card-value {
		font-size: 24px;
		font-weight: 600;
		color: var(--color-text-primary);
		line-height: 1.2;
	}

	.stat-card-secondary {
		font-size: 12px;
		color: var(--color-text-muted);
	}

	.stat-card-sparkline {
		margin-top: 4px;
	}
</style>
