<script lang="ts">
	import type { Toast } from '$lib/types/common';

	let { toast, onRemove }: { toast: Toast; onRemove: () => void } = $props();
</script>

<div
	class="toast toast--{toast.severity}"
	role="alert"
>
	<div class="toast__content">
		<span class="toast__title">{toast.title}</span>
		<span class="toast__message">{toast.message}</span>
	</div>
	<button
		class="toast__close"
		aria-label="Dismiss"
		onclick={onRemove}
	>
		&#10005;
	</button>
</div>
