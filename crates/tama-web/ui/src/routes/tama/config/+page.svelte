<script lang="ts">
	import { getConfig, saveConfig } from '$lib/api/config';
	import type { Config, SamplingParams } from '$lib/types/config';
	import { addToast } from '$lib/stores/toasts';

	let { data } = $props();

	let config = $state<Config>(
		data.config ?? {
			general: { log_level: 'info' },
			proxy: {
				host: '0.0.0.0',
				port: 11434,
				auto_unload: false,
				idle_timeout_secs: 300,
				startup_timeout_secs: 120,
				circuit_breaker_threshold: 3,
				circuit_breaker_cooldown_seconds: 60,
				metrics_retention_secs: 86400
			},
			supervisor: {
				restart_policy: 'always',
				max_restarts: 10,
				restart_delay_ms: 3000,
				health_check_interval_ms: 5000,
				health_check_timeout_ms: 30000,
				health_check_retries: 3
			},
			sampling_templates: {}
		}
	);

	let activeSection = $state<'general' | 'proxy' | 'supervisor' | 'sampling'>('general');
	let loading = $state(!data.config);
	let error = $state<string | null>(data.error ?? null);
	let saving = $state(false);
	let saveStatus = $state<string | null>(null);

	let newTemplateName = $state('');
	let editingTemplate = $state<string | null>(null);
	let newTemplateParams = $state<SamplingParams>({});

	async function handleSave() {
		saving = true;
		saveStatus = 'Saving…';
		error = null;
		try {
			await saveConfig(config);
			saveStatus = '✅ Saved';
			addToast('Success', 'Config saved successfully.', 'success');
			setTimeout(() => (saveStatus = null), 3000);
		} catch (e: any) {
			error = e.message || 'Failed to save config.';
			saveStatus = '❌ Save failed';
			addToast('Error', error ?? 'Failed to save config.', 'error');
		}
		saving = false;
	}

	async function handleRefresh() {
		loading = true;
		error = null;
		try {
			const fresh = await getConfig();
			config = fresh;
		} catch (e: any) {
			error = e.message || 'Failed to refresh config.';
		}
		loading = false;
	}

	function scrollToSection(id: string) {
		const el = document.getElementById(id);
		if (el) {
			el.scrollIntoView({ behavior: 'smooth', block: 'start' });
		}
	}

	function addSamplingTemplate() {
		const name = newTemplateName.trim();
		if (!name) return;
		if (config.sampling_templates[name]) {
			addToast('Error', `Template "${name}" already exists.`, 'error');
			return;
		}
		config.sampling_templates = {
			...config.sampling_templates,
			[name]: {}
		};
		newTemplateName = '';
	}

	function deleteSamplingTemplate(name: string) {
		if (!confirm(`Delete sampling template "${name}"?`)) return;
		const updated = { ...config.sampling_templates };
		delete updated[name];
		config.sampling_templates = updated;
	}

	function startEditTemplate(name: string) {
		editingTemplate = name;
		newTemplateParams = { ...config.sampling_templates[name] };
	}

	function cancelEditTemplate() {
		editingTemplate = null;
		newTemplateParams = {};
	}

	function saveTemplateEdit(name: string) {
		config.sampling_templates = {
			...config.sampling_templates,
			[name]: { ...newTemplateParams }
		};
		editingTemplate = null;
		newTemplateParams = {};
	}

	const sections = [
		{ id: 'general', label: 'General', icon: '⚙️' },
		{ id: 'proxy', label: 'Proxy', icon: '🌐' },
		{ id: 'supervisor', label: 'Supervisor', icon: '👀' },
		{ id: 'sampling', label: 'Sampling Templates', icon: '🎲' }
	];
</script>

<div class="page">
	<div class="page-header">
		<h1>&#9881;&#65039; Configuration</h1>
		<div class="page-header-actions">
			{#if saveStatus}
				<span class="text-sm" style:color={saveStatus?.startsWith('❌') ? 'var(--color-accent-red)' : 'var(--color-accent-green)'}>
					{saveStatus}
				</span>
			{/if}
			<button class="btn btn-secondary" onclick={handleRefresh} disabled={loading}>
				&#8633; Refresh
			</button>
			<button class="btn btn-primary" onclick={handleSave} disabled={saving}>
				{saving ? 'Saving...' : 'Save Changes'}
			</button>
		</div>
	</div>

	{#if error}
		<div class="mb-4 rounded-md bg-accent-red/20 px-4 py-3 text-sm text-accent-red">{error}</div>
	{/if}

	{#if loading}
		<div class="card flex items-center justify-center gap-3 py-12">
			<div class="h-5 w-5 animate-spin rounded-full border-2 border-text-muted border-t-accent-blue"></div>
			<span class="text-muted">Loading config...</span>
		</div>
	{:else}
		<div class="config-layout">
			<!-- Side navigation -->
			<nav class="config-nav card">
				<ul class="config-nav-list">
					{#each sections as section}
						<li>
							<button
								class="config-nav-btn"
								class:active={activeSection === section.id}
								onclick={() => {
									activeSection = section.id as typeof activeSection;
									scrollToSection(`cfg-${section.id}`);
								}}
							>
								<span class="config-nav-icon">{section.icon}</span>
								<span>{section.label}</span>
							</button>
						</li>
					{/each}
				</ul>
			</nav>

			<!-- Main content -->
			<div class="config-content">
				<!-- General Section -->
				<div id="cfg-general" class="config-section card">
					<h2 class="config-section-title">⚙️ General Settings</h2>
					<p class="config-section-desc">Global Tama settings.</p>
					<div class="config-fields">
						<div class="config-field">
							<label class="config-label">Log Level</label>
							<select
								class="config-input"
								value={config.general.log_level}
								onchange={(e) => {
									config.general = { ...config.general, log_level: (e.target as HTMLSelectElement).value };
								}}
							>
								<option value="trace">trace</option>
								<option value="debug">debug</option>
								<option value="info">info</option>
								<option value="warn">warn</option>
								<option value="error">error</option>
							</select>
						</div>

						<div class="config-field">
							<label class="config-label">Models Directory</label>
							<input
								class="config-input"
								type="text"
								placeholder="/path/to/models"
								value={config.general.models_dir ?? ''}
								oninput={(e) => {
									const v = (e.target as HTMLInputElement).value;
									config.general = { ...config.general, models_dir: v || undefined };
								}}
							/>
						</div>

						<div class="config-field">
							<label class="config-label">Logs Directory</label>
							<input
								class="config-input"
								type="text"
								placeholder="/path/to/logs"
								value={config.general.logs_dir ?? ''}
								oninput={(e) => {
									const v = (e.target as HTMLInputElement).value;
									config.general = { ...config.general, logs_dir: v || undefined };
								}}
							/>
						</div>

						<div class="config-field">
							<label class="config-label">HuggingFace Token</label>
							<input
								class="config-input"
								type="password"
								placeholder="hf_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"
								value={config.general.hf_token ?? ''}
								oninput={(e) => {
									const v = (e.target as HTMLInputElement).value;
									config.general = { ...config.general, hf_token: v || undefined };
								}}
							/>
							<p class="config-hint">
								API token for downloading gated models from HuggingFace.
								<a href="https://huggingface.co/settings/tokens" target="_blank" rel="noopener">Get your token</a>
							</p>
						</div>
					</div>
				</div>

				<!-- Proxy Section -->
				<div id="cfg-proxy" class="config-section card">
					<h2 class="config-section-title">🌐 Proxy Settings</h2>
					<p class="config-section-desc">Configure the proxy server that routes OpenAI/Ollama-compatible requests.</p>
					<div class="config-fields">
						<div class="config-field">
							<label class="config-label">Host</label>
							<input
								class="config-input"
								type="text"
								value={config.proxy.host}
								oninput={(e) => {
									config.proxy = { ...config.proxy, host: (e.target as HTMLInputElement).value };
								}}
							/>
						</div>

						<div class="config-field">
							<label class="config-label">Port</label>
							<input
								class="config-input"
								type="number"
								min="1"
								max="65535"
								value={config.proxy.port}
								oninput={(e) => {
									const v = parseInt((e.target as HTMLInputElement).value, 10);
									if (!isNaN(v)) {
										config.proxy = { ...config.proxy, port: v };
									}
								}}
							/>
						</div>

						<div class="config-field config-field-checkbox">
							<label class="config-checkbox-label">
								<input
									type="checkbox"
									checked={config.proxy.auto_unload}
									onchange={(e) => {
										config.proxy = { ...config.proxy, auto_unload: (e.target as HTMLInputElement).checked };
									}}
								/>
								<span>Auto-unload idle models</span>
							</label>
						</div>

						<div class="config-field">
							<label class="config-label">Idle Timeout (seconds)</label>
							<input
								class="config-input"
								type="number"
								min="1"
								value={config.proxy.idle_timeout_secs}
								oninput={(e) => {
									const v = parseInt((e.target as HTMLInputElement).value, 10);
									if (!isNaN(v)) {
										config.proxy = { ...config.proxy, idle_timeout_secs: v };
									}
								}}
							/>
						</div>

						<div class="config-field">
							<label class="config-label">Startup Timeout (seconds)</label>
							<input
								class="config-input"
								type="number"
								min="0"
								value={config.proxy.startup_timeout_secs}
								oninput={(e) => {
									const v = parseInt((e.target as HTMLInputElement).value, 10);
									if (!isNaN(v)) {
										config.proxy = { ...config.proxy, startup_timeout_secs: v };
									}
								}}
							/>
						</div>

						<div class="config-field">
							<label class="config-label">Circuit Breaker Threshold</label>
							<input
								class="config-input"
								type="number"
								min="0"
								value={config.proxy.circuit_breaker_threshold}
								oninput={(e) => {
									const v = parseInt((e.target as HTMLInputElement).value, 10);
									if (!isNaN(v)) {
										config.proxy = { ...config.proxy, circuit_breaker_threshold: v };
									}
								}}
							/>
						</div>

						<div class="config-field">
							<label class="config-label">Circuit Breaker Cooldown (seconds)</label>
							<input
								class="config-input"
								type="number"
								min="0"
								value={config.proxy.circuit_breaker_cooldown_seconds}
								oninput={(e) => {
									const v = parseInt((e.target as HTMLInputElement).value, 10);
									if (!isNaN(v)) {
										config.proxy = { ...config.proxy, circuit_breaker_cooldown_seconds: v };
									}
								}}
							/>
						</div>

						<div class="config-field">
							<label class="config-label">Metrics Retention (seconds)</label>
							<input
								class="config-input"
								type="number"
								min="0"
								value={config.proxy.metrics_retention_secs}
								oninput={(e) => {
									const v = parseInt((e.target as HTMLInputElement).value, 10);
									if (!isNaN(v)) {
										config.proxy = { ...config.proxy, metrics_retention_secs: v };
									}
								}}
							/>
						</div>
					</div>
				</div>

				<!-- Supervisor Section -->
				<div id="cfg-supervisor" class="config-section card">
					<h2 class="config-section-title">👀 Supervisor</h2>
					<p class="config-section-desc">Process restart and health-check behavior for managed models.</p>
					<div class="config-fields">
						<div class="config-field">
							<label class="config-label">Restart Policy</label>
							<select
								class="config-input"
								value={config.supervisor.restart_policy}
								onchange={(e) => {
									config.supervisor = { ...config.supervisor, restart_policy: (e.target as HTMLSelectElement).value };
								}}
							>
								<option value="always">always</option>
								<option value="on-failure">on-failure</option>
								<option value="never">never</option>
							</select>
						</div>

						<div class="config-field">
							<label class="config-label">Max Restarts</label>
							<input
								class="config-input"
								type="number"
								min="0"
								value={config.supervisor.max_restarts}
								oninput={(e) => {
									const v = parseInt((e.target as HTMLInputElement).value, 10);
									if (!isNaN(v)) {
										config.supervisor = { ...config.supervisor, max_restarts: v };
									}
								}}
							/>
						</div>

						<div class="config-field">
							<label class="config-label">Restart Delay (ms)</label>
							<input
								class="config-input"
								type="number"
								min="0"
								value={config.supervisor.restart_delay_ms}
								oninput={(e) => {
									const v = parseInt((e.target as HTMLInputElement).value, 10);
									if (!isNaN(v)) {
										config.supervisor = { ...config.supervisor, restart_delay_ms: v };
									}
								}}
							/>
						</div>

						<div class="config-field">
							<label class="config-label">Health Check Interval (ms)</label>
							<input
								class="config-input"
								type="number"
								min="0"
								value={config.supervisor.health_check_interval_ms}
								oninput={(e) => {
									const v = parseInt((e.target as HTMLInputElement).value, 10);
									if (!isNaN(v)) {
										config.supervisor = { ...config.supervisor, health_check_interval_ms: v };
									}
								}}
							/>
						</div>

						<div class="config-field">
							<label class="config-label">Health Check Timeout (ms)</label>
							<input
								class="config-input"
								type="number"
								min="0"
								value={config.supervisor.health_check_timeout_ms}
								oninput={(e) => {
									const v = parseInt((e.target as HTMLInputElement).value, 10);
									if (!isNaN(v)) {
										config.supervisor = { ...config.supervisor, health_check_timeout_ms: v };
									}
								}}
							/>
						</div>

						<div class="config-field">
							<label class="config-label">Health Check Retries</label>
							<input
								class="config-input"
								type="number"
								min="0"
								value={config.supervisor.health_check_retries}
								oninput={(e) => {
									const v = parseInt((e.target as HTMLInputElement).value, 10);
									if (!isNaN(v)) {
										config.supervisor = { ...config.supervisor, health_check_retries: v };
									}
								}}
							/>
						</div>
					</div>
				</div>

				<!-- Sampling Templates Section -->
				<div id="cfg-sampling" class="config-section card">
					<h2 class="config-section-title">🎲 Sampling Templates</h2>
					<p class="config-section-desc">Reusable named sets of LLM sampling parameters.</p>

					<!-- Add new template -->
					<div class="config-add-template">
						<input
							class="config-input"
							type="text"
							placeholder="Template name (e.g. creative, precise)"
							bind:value={newTemplateName}
							onkeydown={(e) => {
								if (e.key === 'Enter') addSamplingTemplate();
							}}
						/>
						<button class="btn btn-secondary" onclick={addSamplingTemplate}>Add</button>
					</div>

					{#if Object.keys(config.sampling_templates).length === 0}
						<p class="text-muted">No sampling templates defined.</p>
					{:else}
						<div class="config-templates-list">
							{#each Object.entries(config.sampling_templates) as [name, params]}
								<div class="config-template-card">
									<div class="config-template-header">
										<h3 class="config-template-name">{name}</h3>
										<div class="config-template-actions">
											{#if editingTemplate !== name}
												<button class="btn btn-sm btn-secondary" onclick={() => startEditTemplate(name)}>
													Edit
												</button>
												<button class="btn btn-sm btn-danger" onclick={() => deleteSamplingTemplate(name)}>
													Delete
												</button>
											{:else}
												<button class="btn btn-sm btn-primary" onclick={() => saveTemplateEdit(name)}>
													Save
												</button>
												<button class="btn btn-sm btn-secondary" onclick={cancelEditTemplate}>
													Cancel
												</button>
											{/if}
										</div>
									</div>

									{#if editingTemplate === name}
										<div class="config-template-fields">
											<div class="config-template-field">
												<label class="config-label">Temperature</label>
												<input
													class="config-input config-input-sm"
													type="number"
													step="0.01"
													value={newTemplateParams.temperature ?? ''}
													oninput={(e) => {
														const v = (e.target as HTMLInputElement).value;
														newTemplateParams = {
															...newTemplateParams,
															temperature: v === '' ? undefined : parseFloat(v)
														};
													}}
												/>
											</div>
											<div class="config-template-field">
												<label class="config-label">Top K</label>
												<input
													class="config-input config-input-sm"
													type="number"
													min="0"
													step="1"
													value={newTemplateParams.top_k ?? ''}
													oninput={(e) => {
														const v = (e.target as HTMLInputElement).value;
														newTemplateParams = {
															...newTemplateParams,
															top_k: v === '' ? undefined : parseInt(v, 10)
														};
													}}
												/>
											</div>
											<div class="config-template-field">
												<label class="config-label">Top P</label>
												<input
													class="config-input config-input-sm"
													type="number"
													step="0.01"
													value={newTemplateParams.top_p ?? ''}
													oninput={(e) => {
														const v = (e.target as HTMLInputElement).value;
														newTemplateParams = {
															...newTemplateParams,
															top_p: v === '' ? undefined : parseFloat(v)
														};
													}}
												/>
											</div>
											<div class="config-template-field">
												<label class="config-label">Min P</label>
												<input
													class="config-input config-input-sm"
													type="number"
													step="0.01"
													value={newTemplateParams.min_p ?? ''}
													oninput={(e) => {
														const v = (e.target as HTMLInputElement).value;
														newTemplateParams = {
															...newTemplateParams,
															min_p: v === '' ? undefined : parseFloat(v)
														};
													}}
												/>
											</div>
											<div class="config-template-field">
												<label class="config-label">Presence Penalty</label>
												<input
													class="config-input config-input-sm"
													type="number"
													step="0.01"
													value={newTemplateParams.presence_penalty ?? ''}
													oninput={(e) => {
														const v = (e.target as HTMLInputElement).value;
														newTemplateParams = {
															...newTemplateParams,
															presence_penalty: v === '' ? undefined : parseFloat(v)
														};
													}}
												/>
											</div>
											<div class="config-template-field">
												<label class="config-label">Frequency Penalty</label>
												<input
													class="config-input config-input-sm"
													type="number"
													step="0.01"
													value={newTemplateParams.frequency_penalty ?? ''}
													oninput={(e) => {
														const v = (e.target as HTMLInputElement).value;
														newTemplateParams = {
															...newTemplateParams,
															frequency_penalty: v === '' ? undefined : parseFloat(v)
														};
													}}
												/>
											</div>
											<div class="config-template-field">
												<label class="config-label">Repeat Penalty</label>
												<input
													class="config-input config-input-sm"
													type="number"
													step="0.01"
													value={newTemplateParams.repeat_penalty ?? ''}
													oninput={(e) => {
														const v = (e.target as HTMLInputElement).value;
														newTemplateParams = {
															...newTemplateParams,
															repeat_penalty: v === '' ? undefined : parseFloat(v)
														};
													}}
												/>
											</div>
										</div>
									{:else}
										<div class="config-template-preview">
											{#if Object.keys(params).length === 0}
												<span class="text-muted">No parameters set</span>
											{:else}
												{#each Object.entries(params) as [key, value]}
													{#if value !== undefined}
														<span class="config-template-param">
															<span class="config-template-param-key">{key}:</span>
															<span class="config-template-param-value">{value}</span>
														</span>
													{/if}
												{/each}
											{/if}
										</div>
									{/if}
								</div>
							{/each}
						</div>
					{/if}
				</div>
			</div>
		</div>
	{/if}
</div>

<style>
	.config-layout {
		display: flex;
		gap: 1.5rem;
		align-items: flex-start;
	}

	.config-nav {
		width: 220px;
		flex-shrink: 0;
		position: sticky;
		top: 1rem;
		padding: 0.75rem;
	}

	.config-nav-list {
		list-style: none;
		padding: 0;
		margin: 0;
		display: flex;
		flex-direction: column;
		gap: 0.25rem;
	}

	.config-nav-btn {
		width: 100%;
		text-align: left;
		display: flex;
		gap: 0.5rem;
		align-items: center;
		padding: 0.5rem 0.75rem;
		border: none;
		border-radius: 0.375rem;
		background: transparent;
		color: var(--color-text-secondary);
		font-size: 0.875rem;
		cursor: pointer;
		transition: all 0.15s;
	}

	.config-nav-btn:hover {
		background: var(--color-bg-tertiary);
		color: var(--color-text-primary);
	}

	.config-nav-btn.active {
		background: var(--color-accent-blue);
		color: white;
	}

	.config-nav-icon {
		font-size: 1rem;
	}

	.config-content {
		flex: 1;
		min-width: 0;
		display: flex;
		flex-direction: column;
		gap: 1rem;
	}

	.config-section {
		padding: 1.25rem;
	}

	.config-section-title {
		margin: 0 0 0.25rem 0;
		font-size: 1.125rem;
		font-weight: 600;
		color: var(--color-text-primary);
	}

	.config-section-desc {
		margin: 0 0 1rem 0;
		font-size: 0.875rem;
		color: var(--color-text-muted);
	}

	.config-fields {
		display: flex;
		flex-direction: column;
		gap: 1rem;
	}

	.config-field {
		display: flex;
		flex-direction: column;
		gap: 0.25rem;
	}

	.config-field-checkbox {
		flex-direction: row;
		align-items: center;
	}

	.config-label {
		font-size: 0.8125rem;
		font-weight: 500;
		color: var(--color-text-secondary);
	}

	.config-input {
		padding: 0.375rem 0.625rem;
		border: 1px solid var(--color-border-default);
		border-radius: 0.375rem;
		background: var(--color-bg-primary);
		color: var(--color-text-primary);
		font-size: 0.875rem;
		outline: none;
		transition: border-color 0.15s;
	}

	.config-input:focus {
		border-color: var(--color-accent-blue);
		box-shadow: 0 0 0 2px rgba(88, 166, 255, 0.2);
	}

	.config-input-sm {
		padding: 0.25rem 0.5rem;
		font-size: 0.8125rem;
	}

	.config-hint {
		font-size: 0.75rem;
		color: var(--color-text-muted);
		margin-top: 0.125rem;
	}

	.config-hint a {
		color: var(--color-accent-blue);
		text-decoration: none;
	}

	.config-hint a:hover {
		text-decoration: underline;
	}

	.config-checkbox-label {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		font-size: 0.875rem;
		color: var(--color-text-primary);
		cursor: pointer;
	}

	/* Sampling templates */
	.config-add-template {
		display: flex;
		gap: 0.5rem;
		margin-bottom: 1rem;
	}

	.config-add-template .config-input {
		flex: 1;
	}

	.config-templates-list {
		display: flex;
		flex-direction: column;
		gap: 0.75rem;
	}

	.config-template-card {
		border: 1px solid var(--color-border-default);
		border-radius: 0.5rem;
		padding: 0.75rem;
	}

	.config-template-header {
		display: flex;
		justify-content: space-between;
		align-items: center;
		margin-bottom: 0.5rem;
	}

	.config-template-name {
		margin: 0;
		font-size: 0.9375rem;
		font-weight: 600;
		color: var(--color-text-primary);
	}

	.config-template-actions {
		display: flex;
		gap: 0.25rem;
	}

	.btn-sm {
		padding: 0.2rem 0.5rem;
		font-size: 0.75rem;
	}

	.btn-danger {
		background: var(--color-accent-red);
		color: white;
	}

	.btn-danger:hover {
		background: var(--color-accent-red) !important;
		opacity: 0.85;
	}

	.config-template-fields {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: 0.5rem;
	}

	.config-template-field {
		display: flex;
		flex-direction: column;
		gap: 0.125rem;
	}

	.config-template-preview {
		display: flex;
		flex-wrap: wrap;
		gap: 0.5rem;
	}

	.config-template-param {
		font-size: 0.8125rem;
	}

	.config-template-param-key {
		color: var(--color-text-muted);
	}

	.config-template-param-value {
		color: var(--color-accent-cyan);
		font-weight: 500;
	}

	@media (max-width: 768px) {
		.config-layout {
			flex-direction: column;
		}

		.config-nav {
			width: 100%;
			position: static;
		}

		.config-nav-list {
			flex-direction: row;
			flex-wrap: wrap;
		}

		.config-nav-btn {
			width: auto;
		}

		.config-template-fields {
			grid-template-columns: 1fr;
		}
	}
</style>
