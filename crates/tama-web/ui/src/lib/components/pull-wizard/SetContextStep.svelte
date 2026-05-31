<script lang="ts">
	import { CACHE_TYPES } from '$lib/types/model-editor';

	interface Props {
		contextLength: number | undefined;
		kvUnified: boolean;
		cacheTypeK: string | undefined;
		cacheTypeV: string | undefined;
		onContextLengthChange: (val: number | undefined) => void;
		onKvUnifiedChange: (val: boolean) => void;
		onCacheTypeKChange: (val: string | undefined) => void;
		onCacheTypeVChange: (val: string | undefined) => void;
		onFinish: () => void;
	}

	let {
		contextLength,
		kvUnified,
		cacheTypeK,
		cacheTypeV,
		onContextLengthChange,
		onKvUnifiedChange,
		onCacheTypeKChange,
		onCacheTypeVChange,
		onFinish
	}: Props = $props();
</script>

<div class="space-y-4">
	<h4 class="text-sm font-medium text-text-primary">Configure Context Settings</h4>
	<p class="text-xs text-text-muted">These settings will be applied to the new model configuration.</p>

	<!-- Context Length -->
	<div>
		<label class="mb-1 block text-sm font-medium text-text-primary" for="wizard-context-length">Context Length</label>
		<input
			id="wizard-context-length"
			type="number"
			min="0"
			class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
			placeholder="Auto"
			value={contextLength?.toString() ?? ''}
			oninput={(e: Event) => {
				const val = (e.target as HTMLInputElement).value;
				onContextLengthChange(val === '' ? undefined : parseInt(val, 10));
			}}
		/>
	</div>

	<!-- KV Unified -->
	<div class="flex items-center gap-2">
		<input
			id="wizard-kv-unified"
			type="checkbox"
			class="rounded border-border-default bg-bg-primary text-accent-blue focus:ring-accent-blue/50"
			checked={kvUnified}
			onchange={(e: Event) => onKvUnifiedChange((e.target as HTMLInputElement).checked)}
		/>
		<label class="text-sm text-text-primary" for="wizard-kv-unified">KV Unified</label>
	</div>

	<!-- Cache Type K -->
	<div>
		<label class="mb-1 block text-sm font-medium text-text-primary" for="wizard-cache-k">Cache Type K</label>
		<select
			id="wizard-cache-k"
			class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
			value={cacheTypeK ?? ''}
			onchange={(e: Event) => {
				const val = (e.target as HTMLSelectElement).value;
				onCacheTypeKChange(val || undefined);
			}}
		>
			<option value="">Auto</option>
			{#each CACHE_TYPES as ct (ct)}
				<option value={ct}>{ct}</option>
			{/each}
		</select>
	</div>

	<!-- Cache Type V -->
	<div>
		<label class="mb-1 block text-sm font-medium text-text-primary" for="wizard-cache-v">Cache Type V</label>
		<select
			id="wizard-cache-v"
			class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
			value={cacheTypeV ?? ''}
			onchange={(e: Event) => {
				const val = (e.target as HTMLSelectElement).value;
				onCacheTypeVChange(val || undefined);
			}}
		>
			<option value="">Auto</option>
			{#each CACHE_TYPES as ct (ct)}
				<option value={ct}>{ct}</option>
			{/each}
		</select>
	</div>

	<!-- Finish button -->
	<div class="flex justify-end pt-2">
		<button class="btn btn-primary" onclick={onFinish}>
			Finish
		</button>
	</div>
</div>
