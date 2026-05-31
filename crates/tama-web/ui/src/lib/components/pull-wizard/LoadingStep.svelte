<script lang="ts">
	interface Props {
		repoId: string;
	}

	let { repoId }: Props = $props();
</script>

<div class="flex flex-col items-center justify-center py-8 gap-3">
	<div class="h-8 w-8 animate-spin rounded-full border-3 border-text-muted border-t-accent-blue"></div>
	<div class="text-center">
		<p class="text-text-primary font-medium">Loading repo metadata...</p>
		<p class="text-sm text-text-muted mt-1 font-mono">{repoId}</p>
	</div>
</div>
