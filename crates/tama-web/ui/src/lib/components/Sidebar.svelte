<script lang="ts">
	import { onMount } from 'svelte';
	import { browser } from '$app/environment';

	let collapsed = $state(false);
	let mobileOpen = $state(false);
	let updateBadgeVisible = $state(false);

	onMount(() => {
		// Read persisted collapsed state
		if (browser) {
			const stored = localStorage.getItem('tama-sidebar-collapsed');
			if (stored === 'true') collapsed = true;
		}

		// Fetch updates badge
		fetch('/tama/v1/updates', { credentials: 'same-origin' })
			.then((r) => r.json())
			.then((data) => {
				const hasUpdates =
					(data.backends || []).some(
						(b: { update_available?: boolean }) => b.update_available
					) ||
					(data.models || []).some(
						(m: { update_available?: boolean }) => m.update_available
					);
				updateBadgeVisible = hasUpdates;
			})
			.catch(() => {
				/* ignore */
			});
	});

	$effect(() => {
		if (browser) {
			localStorage.setItem('tama-sidebar-collapsed', String(collapsed));
		}
	});
</script>

<!-- Mobile hamburger -->
<button class="sidebar-mobile-toggle" onclick={() => (mobileOpen = true)}>
	&#9776;
</button>

<!-- Overlay backdrop -->
{#if mobileOpen}
	<!-- svelte-ignore a11y_click_events_have_key_events -->
	<!-- svelte-ignore a11y_no_static_element_interactions -->
	<div
		class="sidebar-overlay sidebar-overlay--visible"
		onclick={() => (mobileOpen = false)}
	></div>
{/if}

<aside class="sidebar" class:sidebar--collapsed={collapsed} class:sidebar--mobile-open={mobileOpen}>
	<!-- Close button for mobile -->
	<button class="sidebar-close" onclick={() => (mobileOpen = false)}>&#10005;</button>

	<a
		href="/tama"
		class="sidebar-header"
		onclick={() => (mobileOpen = false)}
	>
		<span class="sidebar-header__logo">&#129433;</span>
		<span class="sidebar-header__text">Tama</span>
	</a>

	<nav class="sidebar-nav">
		<a
			href="/tama"
			class="sidebar-item"
			data-tooltip="Dashboard"
			onclick={() => (mobileOpen = false)}
		>
			<span class="sidebar-item__icon">&#127968;</span>
			<span class="sidebar-item__text">Dashboard</span>
		</a>
		<a
			href="/tama/backends"
			class="sidebar-item"
			data-tooltip="Backends"
			onclick={() => (mobileOpen = false)}
		>
			<span class="sidebar-item__icon">&#128295;</span>
			<span class="sidebar-item__text">Backends</span>
		</a>
		<a
			href="/tama/logs"
			class="sidebar-item"
			data-tooltip="Logs"
			onclick={() => (mobileOpen = false)}
		>
			<span class="sidebar-item__icon">&#128203;</span>
			<span class="sidebar-item__text">Logs</span>
		</a>
		<a
			href="/tama/updates"
			class="sidebar-item"
			data-tooltip="Updates"
			onclick={() => (mobileOpen = false)}
		>
			<span class="sidebar-item__icon">&#128260;</span>
			<span class="sidebar-item__text">Updates</span>
			{#if updateBadgeVisible}
				<span class="sidebar-badge">!</span>
			{/if}
		</a>
		<a
			href="/tama/downloads"
			class="sidebar-item"
			data-tooltip="Downloads"
			onclick={() => (mobileOpen = false)}
		>
			<span class="sidebar-item__icon">&#128229;</span>
			<span class="sidebar-item__text">Downloads</span>
		</a>
		<a
			href="/tama/benchmarks"
			class="sidebar-item"
			data-tooltip="Benchmarks"
			onclick={() => (mobileOpen = false)}
		>
			<span class="sidebar-item__icon">&#128202;</span>
			<span class="sidebar-item__text">Benchmarks</span>
		</a>
		<a
			href="/tama/aliases"
			class="sidebar-item"
			data-tooltip="Aliases"
			onclick={() => (mobileOpen = false)}
		>
			<span class="sidebar-item__icon">&#127991;&#65039;</span>
			<span class="sidebar-item__text">Aliases</span>
		</a>
	</nav>

	<div class="sidebar-footer">
		<div class="sidebar-section" style="border-top:none;margin:0;padding:0;">
			<a
				href="/tama/config"
				class="sidebar-item"
				data-tooltip="Config"
				onclick={() => (mobileOpen = false)}
			>
				<span class="sidebar-item__icon">&#9881;&#65039;</span>
				<span class="sidebar-item__text">Config</span>
			</a>
		</div>

		<button class="sidebar-toggle" onclick={() => (collapsed = !collapsed)}>
			<span class="sidebar-toggle__icon">&#8596;</span>
			<span class="sidebar-toggle__text">Collapse</span>
		</button>
	</div>
</aside>
