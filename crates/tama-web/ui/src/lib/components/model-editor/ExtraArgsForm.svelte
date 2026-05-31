<script lang="ts">
	import type { ModelForm } from '$lib/types/model-editor';

	interface Props {
		form: ModelForm;
	}

	let { form }: Props = $props();
</script>

<div>
	<label class="mb-1 block text-sm font-medium text-text-primary" for="extra-args">Extra Arguments</label>
	<textarea
		id="extra-args"
		class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-blue/50 font-mono"
		placeholder="-ngl 32&#10;-c 8192&#10;-m /path/to/model.gguf"
		rows={8}
		bind:value={form.args}
	></textarea>
	<p class="mt-1 text-xs text-text-muted">One argument per line. Empty lines are ignored.</p>
</div>
