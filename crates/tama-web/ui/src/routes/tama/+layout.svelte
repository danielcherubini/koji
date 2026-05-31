<script lang="ts">
	import { onMount } from 'svelte';
	import Sidebar from '$lib/components/Sidebar.svelte';
	import ToastContainer from '$lib/components/ToastContainer.svelte';
	import { startDownloadEvents, stopDownloadEvents } from '$lib/stores/downloads';
	import type { Snippet } from 'svelte';

	let { children }: { children: Snippet } = $props();

	onMount(() => {
		startDownloadEvents();
		return () => stopDownloadEvents();
	});
</script>

<Sidebar />
<ToastContainer />

<main class="main-content">
	{@render children()}
</main>
