<script lang="ts">
	import type { ModelForm } from '$lib/types/model-editor';
	import { SPEC_TYPES } from '$lib/types/model-editor';

	interface Props {
		form: ModelForm;
	}

	let { form }: Props = $props();

	function toggleSpecType(type: string) {
		const types = [...(form.spec_decoding?.spec_types ?? [])];
		const idx = types.indexOf(type);
		if (idx >= 0) {
			types.splice(idx, 1);
		} else {
			types.push(type);
		}
		form.spec_decoding = {
			...form.spec_decoding,
			spec_types: types
		};
	}

	function setSpecNumber(key: 'n_max' | 'n_min' | 'draft_ngl', value: string) {
		const num = value === '' ? undefined : parseInt(value, 10);
		form.spec_decoding = {
			...form.spec_decoding,
			[key]: isNaN(num ?? NaN) ? undefined : num
		};
	}
</script>

<div class="space-y-4">
	<!-- Spec types checkboxes -->
	<div>
		<p class="mb-2 block text-sm font-medium text-text-primary">Spec Types</p>
		<div class="space-y-2">
			{#each SPEC_TYPES as type}
				<div class="flex items-center gap-2">
					<input
						id={`spec-${type}`}
						type="checkbox"
						class="rounded border-border-default bg-bg-primary text-accent-blue focus:ring-accent-blue/50"
						checked={(form.spec_decoding?.spec_types ?? []).includes(type)}
						onchange={() => toggleSpecType(type)}
					/>
					<label class="text-sm text-text-primary" for={`spec-${type}`}>{type}</label>
				</div>
			{/each}
		</div>
	</div>

	<!-- Number inputs -->
	<div class="grid grid-cols-1 md:grid-cols-3 gap-4">
		<div>
			<label class="mb-1 block text-sm font-medium text-text-primary" for="n-max">N Max</label>
			<input
				id="n-max"
				type="number"
				min="0"
				class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
				placeholder="Auto"
				value={form.spec_decoding?.n_max?.toString() ?? ''}
				oninput={(e: Event) => setSpecNumber('n_max', (e.target as HTMLInputElement).value)}
			/>
		</div>

		<div>
			<label class="mb-1 block text-sm font-medium text-text-primary" for="n-min">N Min</label>
			<input
				id="n-min"
				type="number"
				min="0"
				class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
				placeholder="Auto"
				value={form.spec_decoding?.n_min?.toString() ?? ''}
				oninput={(e: Event) => setSpecNumber('n_min', (e.target as HTMLInputElement).value)}
			/>
		</div>

		<div>
			<label class="mb-1 block text-sm font-medium text-text-primary" for="draft-ngl">Draft NGL</label>
			<input
				id="draft-ngl"
				type="number"
				min="0"
				class="w-full rounded-md border border-border-default bg-bg-primary px-3 py-2 text-sm text-text-primary placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
				placeholder="Auto"
				value={form.spec_decoding?.draft_ngl?.toString() ?? ''}
				oninput={(e: Event) => setSpecNumber('draft_ngl', (e.target as HTMLInputElement).value)}
			/>
		</div>
	</div>
</div>
