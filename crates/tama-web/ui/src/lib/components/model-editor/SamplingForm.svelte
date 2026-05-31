<script lang="ts">
	import type { ModelForm, SamplingField } from '$lib/types/model-editor';
	import { SAMPLING_FIELDS } from '$lib/types/model-editor';

	interface Props {
		form: ModelForm;
		samplingTemplates: Record<string, Record<string, unknown>> | null;
	}

	let { form, samplingTemplates }: Props = $props();

	// Available preset names
	let presetNames = $derived(
		samplingTemplates ? Object.keys(samplingTemplates) : []
	);

	// Sampling field definitions with labels and defaults
	const fieldDefs = [
		{ key: 'temperature', label: 'Temperature', default: '0.8', type: 'number' },
		{ key: 'top_k', label: 'Top K', default: '40', type: 'number' },
		{ key: 'top_p', label: 'Top P', default: '0.9', type: 'number' },
		{ key: 'min_p', label: 'Min P', default: '0.05', type: 'number' },
		{ key: 'presence_penalty', label: 'Presence Penalty', default: '0.0', type: 'number' },
		{ key: 'frequency_penalty', label: 'Frequency Penalty', default: '0.0', type: 'number' },
		{ key: 'repeat_penalty', label: 'Repeat Penalty', default: '1.1', type: 'number' }
	] as const;

	function getField(key: string): SamplingField {
		return form.sampling[key] ?? { enabled: false, value: '' };
	}

	function setFieldEnabled(key: string, enabled: boolean) {
		if (!form.sampling[key]) {
			form.sampling[key] = { enabled, value: '' };
		} else {
			form.sampling[key].enabled = enabled;
		}
	}

	function setFieldValue(key: string, value: string) {
		if (!form.sampling[key]) {
			form.sampling[key] = { enabled: true, value };
		} else {
			form.sampling[key].value = value;
		}
	}

	function loadPreset(presetName: string) {
		if (!samplingTemplates || !presetName) return;
		const preset = samplingTemplates[presetName];
		if (!preset) return;

		for (const def of fieldDefs) {
			const val = preset[def.key];
			if (val !== undefined && val !== null) {
				form.sampling[def.key] = {
					enabled: true,
					value: String(val)
				};
			}
		}
	}
</script>

<div class="space-y-4">
	<!-- Preset selector -->
	{#if presetNames.length > 0}
		<div class="flex items-center gap-3 mb-4">
			<label class="text-sm font-medium text-text-primary" for="preset">Load Preset</label>
			<select
				id="preset"
				class="rounded-md border border-border-default bg-bg-primary px-3 py-1.5 text-sm text-text-primary focus:outline-none focus:ring-2 focus:ring-accent-blue/50"
				onchange={(e: Event) => {
					const val = (e.target as HTMLSelectElement).value;
					if (val) loadPreset(val);
				}}
			>
				<option value="">-- Select preset --</option>
				{#each presetNames as name (name)}
					<option value={name}>{name}</option>
				{/each}
			</select>
		</div>
	{/if}

	<!-- Sampling fields -->
	<div class="grid grid-cols-1 md:grid-cols-2 gap-4">
		{#each fieldDefs as def}
			<div class="flex items-center gap-3">
				<!-- Checkbox -->
				<div class="flex items-center gap-2 shrink-0">
					<input
						id={`sampling-${def.key}`}
						type="checkbox"
						class="rounded border-border-default bg-bg-primary text-accent-blue focus:ring-accent-blue/50"
						checked={getField(def.key).enabled}
						onchange={(e: Event) => {
							setFieldEnabled(def.key, (e.target as HTMLInputElement).checked);
							if ((e.target as HTMLInputElement).checked && !getField(def.key).value) {
								setFieldValue(def.key, def.default);
							}
						}}
					/>
					<label class="text-sm text-text-primary" for={`sampling-${def.key}`}>{def.label}</label>
				</div>
				<!-- Value input -->
				<input
					type={def.type}
					class="w-28 rounded-md border border-border-default bg-bg-primary px-2 py-1 text-sm text-text-primary focus:outline-none focus:ring-2 focus:ring-accent-blue/50 disabled:opacity-50"
					value={getField(def.key).value}
					disabled={!getField(def.key).enabled}
					oninput={(e: Event) => {
						setFieldValue(def.key, (e.target as HTMLInputElement).value);
					}}
				/>
			</div>
		{/each}
	</div>
</div>
