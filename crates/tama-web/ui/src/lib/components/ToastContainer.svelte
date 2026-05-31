<script lang="ts">
	import { toasts } from '$lib/stores/toasts';
	import ToastCard from './ToastCard.svelte';
</script>

<div class="toast-container">
	{#each $toasts as toast (toast.id)}
		<ToastCard {toast} onRemove={() => toasts.update((t) => t.filter((x) => x.id !== toast.id))} />
	{/each}
</div>
