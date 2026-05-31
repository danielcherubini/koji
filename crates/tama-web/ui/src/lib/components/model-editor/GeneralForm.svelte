<script lang="ts">
	import type { ModelForm, BackendOption, QuantInfo } from '$lib/types/model-editor';
	import { CACHE_TYPES } from '$lib/types/model-editor';

	interface Props {
		form: ModelForm;
		backends: BackendOption[];
	}

	let { form, backends }: Props = $props();

	// GPU variants derived from selected backend
	let gpuVariants = $derived(
		backends
			.filter((b) => b.name === form.backend)
			.flatMap((b) => (b.variant ? [b.variant] : []))
	);

	// Quant options from form.quants (model kind only)
	let quantOptions = $derived(
		Object.entries(form.quants ?? {})
			.filter(([, q]) => q.kind === 'model')
			.map(([key, q]) => ({ key, label: q.file || key }))
	);

	// MMProj options from form.quants (mmproj kind)
	let mmprojOptions = $derived(
		Object.entries(form.quants ?? {})
			.filter(([, q]) => q.kind === 'mmproj')
			.map(([key, q]) => ({ key, label: q.file || key }))
	);

	function handleBackendChange() {
		// Clear gpu_variant when backend changes
		if (form.gpu_variant && !gpuVariants.includes(form.gpu_variant)) {
			form.gpu_variant = undefined;
		}
	}
</script>

<div class="grid grid-cols-1 md:grid-cols-2 gap-4">
	<!-- Backend -->
	<div>
		<label class="mb-1 block text-sm font-medium text-text-primary" for="backend">Backend</label>
		<select
			id="backend"
			class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
			bind:value={form.backend}
			onchange={handleBackendChange}
		>
			{#each backends as backend (backend.name + (backend.variant ?? ''))}
				<option value={backend.name}>{backend.label || backend.name}</option>
			{/each}
		</select>
	</div>

	<!-- GPU Variant -->
	<div>
		<label class="mb-1 block text-sm font-medium text-text-primary" for="gpu-variant">GPU Variant</label>
		<select
			id="gpu-variant"
			class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
			bind:value={form.gpu_variant}
		>
			<option value="">Auto</option>
			{#each gpuVariants as variant (variant)}
				<option value={variant}>{variant}</option>
			{/each}
		</select>
	</div>

	<!-- Model path -->
	<div>
		<label class="mb-1 block text-sm font-medium text-text-primary" for="model-path">Model Path</label>
		<input
			id="model-path"
			type="text"
			class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
			placeholder="/path/to/model"
			bind:value={form.model}
		/>
	</div>

	<!-- Quant -->
	<div>
		<label class="mb-1 block text-sm font-medium text-text-primary" for="quant">Quant</label>
		<select
			id="quant"
			class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
			bind:value={form.quant}
		>
			<option value="">-- Select quant --</option>
			{#each quantOptions as opt (opt.key)}
				<option value={opt.key}>{opt.label}</option>
			{/each}
		</select>
	</div>

	<!-- MMProj -->
	<div>
		<label class="mb-1 block text-sm font-medium text-text-primary" for="mmproj">MMProj</label>
		<select
			id="mmproj"
			class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
			bind:value={form.mmproj}
		>
			<option value="">-- Select mmproj --</option>
			{#each mmprojOptions as opt (opt.key)}
				<option value={opt.key}>{opt.label}</option>
			{/each}
		</select>
	</div>

	<!-- Context Length -->
	<div>
		<label class="mb-1 block text-sm font-medium text-text-primary" for="context-length">Context Length</label>
		<input
			id="context-length"
			type="number"
			min="0"
			class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
			placeholder="Auto"
			bind:value={form.context_length}
		/>
	</div>

	<!-- Num Parallel -->
	<div>
		<label class="mb-1 block text-sm font-medium text-text-primary" for="num-parallel">Num Parallel</label>
		<input
			id="num-parallel"
			type="number"
			min="0"
			class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
			placeholder="Auto"
			bind:value={form.num_parallel}
		/>
		<p class="mt-1 text-xs text-text-muted">0 = auto</p>
	</div>

	<!-- KV Unified -->
	<div class="flex items-center gap-2 pt-6">
		<input
			id="kv-unified"
			type="checkbox"
			class="rounded border-border-default bg-bg-primary text-accent-blue focus:ring-accent-blue/50"
			bind:checked={form.kv_unified}
		/>
		<label class="text-sm text-text-primary" for="kv-unified">KV Unified</label>
	</div>

	<!-- Port -->
	<div>
		<label class="mb-1 block text-sm font-medium text-text-primary" for="port">Port</label>
		<input
			id="port"
			type="number"
			min="1"
			max="65535"
			class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
			placeholder="Auto"
			bind:value={form.port}
		/>
	</div>

	<!-- API Name -->
	<div>
		<label class="mb-1 block text-sm font-medium text-text-primary" for="api-name">API Name</label>
		<input
			id="api-name"
			type="text"
			class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
			placeholder="Model ID"
			bind:value={form.api_name}
		/>
	</div>

	<!-- Display Name -->
	<div>
		<label class="mb-1 block text-sm font-medium text-text-primary" for="display-name">Display Name</label>
		<input
			id="display-name"
			type="text"
			class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
			placeholder="Friendly name"
			bind:value={form.display_name}
		/>
	</div>

	<!-- GPU Layers -->
	<div>
		<label class="mb-1 block text-sm font-medium text-text-primary" for="gpu-layers">GPU Layers</label>
		<input
			id="gpu-layers"
			type="number"
			min="0"
			class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
			placeholder="Auto"
			bind:value={form.gpu_layers}
		/>
	</div>

	<!-- Cache Type K -->
	<div>
		<label class="mb-1 block text-sm font-medium text-text-primary" for="cache-type-k">Cache Type K</label>
		<select
			id="cache-type-k"
			class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
			bind:value={form.cache_type_k}
		>
			<option value="">Auto</option>
			{#each CACHE_TYPES as ct (ct)}
				<option value={ct}>{ct}</option>
			{/each}
		</select>
	</div>

	<!-- Cache Type V -->
	<div>
		<label class="mb-1 block text-sm font-medium text-text-primary" for="cache-type-v">Cache Type V</label>
		<select
			id="cache-type-v"
			class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
			bind:value={form.cache_type_v}
		>
			<option value="">Auto</option>
			{#each CACHE_TYPES as ct (ct)}
				<option value={ct}>{ct}</option>
			{/each}
		</select>
	</div>
</div>
