# SvelteKit Frontend Migration Spec

**Date**: 2026-05-31
**Status**: 📋 DRAFT
**Author**: AI (coding agent)
**Category**: Web UI / Architecture

---

## Goal

Replace the Leptos/WASM frontend with a SvelteKit + TypeScript + Tailwind CSS frontend. Reduce initial load from 5.1MB (WASM) to ~120KB (JS, uncompressed). Improve developer experience, debugging, and long-term maintainability.

## Background

The current frontend is built with [Leptos 0.7](https://leptos.dev) — a Rust web framework that compiles to WebAssembly. After ~1.5 years of development, the codebase has grown to ~29,268 lines of Rust across 22 components, 10 pages, and 29 API module files. The compiled WASM binary is 5.1MB uncompressed (~1.5MB gzipped with compression).

### Why Migrate

1. **WASM is the wrong tool for this job** — Tama is a CRUD admin dashboard. Every page either fetches JSON and renders it, listens to SSE streams, or submits forms. No computation justifies WASM.
2. **5.1MB initial load** — Even with gzip, ~1.5MB is excessive for a localhost admin panel. No code splitting, no lazy loading.
3. **Rust-JS interop tax** — SSE EventSource setup is ~100 lines of `Closure::wrap` + `unchecked_ref` + `forget` boilerplate. Form binding requires manual `dyn_into::<HtmlInputElement>()` casting.
4. **No CSS story** — 18 hand-written CSS files, no component scoping, no utility framework. Leptos provides zero CSS tooling.
5. **Illusory type sharing** — `#[cfg(feature = "ssr")]` splits everywhere. CSR side defines its own types mirroring the server. Functions like `infer_quant_from_filename` are literally duplicated.
6. **AI assistant training data gap** — The coding agent has significantly more training data for Svelte/React/Vue than Leptos, leading to faster, more reliable feature delivery and debugging.

## Architecture

### Stack

| Layer | Technology |
|-------|-----------|
| Framework | SvelteKit 2 (static output) |
| Language | TypeScript (strict mode) |
| Styling | Tailwind CSS v4 with custom dark theme |
| Build | Vite |
| Package Manager | pnpm |
| Routing | SvelteKit file-based routing |
| State | Svelte 5 runes (`$state`, `$effect`, `$derived`) + writable stores |
| HTTP | Native `fetch()` with thin CSRF wrapper |
| SSE | Native `EventSource` |

### Directory Structure

```
tama/
├── crates/
│   ├── tama-core/        # Unchanged
│   ├── tama-cli/         # Minimal change — update include_dir! path
│   ├── tama-mock/        # Unchanged
│   └── tama-web/          # Split: API routes (Rust) + UI (SvelteKit)
│       ├── src/           # KEEP — Axum API handlers (ssr feature)
│       │   ├── api/
│       │   ├── router.rs  # build_web_routes()
│       │   ├── jobs.rs
│       │   ├── gpu.rs
│       │   └── lib.rs     # Trimmed — SSR-only exports
│       ├── Cargo.toml     # Trimmed — remove Leptos/wasm deps
│       └── ui/            # NEW — SvelteKit application
│           ├── src/
│           │   ├── routes/
│           │   │   └── tama/
│           │   │       ├── +layout.svelte      # Sidebar + ToastContainer
│           │   │       ├── +page.svelte         # Dashboard
│           │   │       ├── models/
│           │   │       ├── model/[id]/edit/
│           │   │       ├── backends/
│           │   │       ├── benchmarks/
│           │   │       ├── aliases/
│           │   │       ├── logs/
│           │   │       ├── config/
│           │   │       ├── updates/
│           │   │       └── downloads/
│           │   ├── lib/
│           │   │   ├── components/
│           │   │   ├── stores/
│           │   │   ├── api/
│           │   │   └── types/
│           │   └── app.html
│           ├── static/
│           ├── package.json
│           ├── svelte.config.js
│           ├── vite.config.ts
│           ├── tailwind.config.ts
│           └── tsconfig.json
```

### Build Pipeline

**Current:**
```makefile
build-frontend: wasm-target
	cd crates/tama-web && trunk build --release --public-url /tama
```

**Proposed:**
```makefile
build-frontend:
	cd crates/tama-web/ui && pnpm install && pnpm build
```

SvelteKit outputs to `ui/build/`. The `include_dir!` in `router.rs` updates from:
```rust
static DIST: Dir = include_dir!("$CARGO_MANIFEST_DIR/dist");
```
to:
```rust
static DIST: Dir = include_dir!("$CARGO_MANIFEST_DIR/ui/build");
```

### API Boundary

The Rust API (`/tama/v1/*`) stays unchanged. The SvelteKit frontend calls it via `fetch()`:

```typescript
// src/lib/api/client.ts
export async function apiFetch(path: string, options: RequestInit = {}) {
  const token = getCsrfToken();
  const headers: HeadersInit = {
    'Content-Type': 'application/json',
    ...(token ? { 'X-CSRF-Token': token } : {}),
    ...options.headers,
  };
  const resp = await fetch(`/tama/v1${path}`, { ...options, headers });
  const csrfFromHeader = resp.headers.get('X-CSRF-Token');
  if (csrfFromHeader) storeCsrfToken(csrfFromHeader);
  return resp;
}

export const api = {
  get: (path: string) => apiFetch(path, { method: 'GET' }),
  post: (path: string, body?: unknown) => apiFetch(path, {
    method: 'POST',
    body: body ? JSON.stringify(body) : undefined,
  }),
  put: (path: string, body?: unknown) => apiFetch(path, {
    method: 'PUT',
    body: body ? JSON.stringify(body) : undefined,
  }),
  delete: (path: string) => apiFetch(path, { method: 'DELETE' }),
};
```

### CSS Strategy

**Tailwind CSS v4** with custom dark theme configuration:

```typescript
// tailwind.config.ts
export default {
  darkMode: 'class',
  theme: {
    extend: {
      colors: {
        bg: {
          primary: '#1a1b1e',
          secondary: '#25262b',
          tertiary: '#2c2e33',
        },
        accent: {
          green: '#4ade80',
          blue: '#60a5fa',
          yellow: '#fbbf24',
          red: '#f87171',
          purple: '#a78bfa',
          orange: '#fb923c',
          cyan: '#22d3ee',
          pink: '#f472b6',
        },
      },
    },
  },
};
```

Repeated patterns (buttons, cards, badges) defined once in `@layer components`:

```css
/* src/app.css */
@import "tailwindcss";

@layer components {
  .btn {
    @apply inline-flex items-center justify-center rounded-md px-4 py-2
           text-sm font-medium transition-colors focus:outline-none focus:ring-2;
  }
  .btn-primary { @apply bg-accent-blue text-white hover:bg-accent-blue/80; }
  .btn-secondary { @apply bg-bg-tertiary text-text-primary hover:bg-bg-tertiary/80; }
  .btn-danger { @apply bg-accent-red text-white hover:bg-accent-red/80; }
  .card { @apply bg-bg-secondary rounded-lg p-4 border border-white/5; }
  .badge { @apply inline-flex items-center rounded-full px-2.5 py-0.5 text-xs font-medium; }
}
```

SVG-specific styles (sparkline hover, tooltips) use Svelte's scoped `<style>` blocks.

## Pattern Migrations

### State Management

| Concern | Leptos (Current) | SvelteKit (Proposed) |
|---------|-------------------|---------------------|
| Local state | `RwSignal::new(T)` | `let x = $state(initialValue)` |
| Global state | `static X: RwSignal<T>` | `export const x = writable<T>(initial)` |
| Derived | `Signal::derive(\|\| ...)` | `let x = $derived(...)` |
| Side effects | `Effect::new(\|_|\| { ... })` | `$effect(() => { ... })` |
| Cleanup | `on_cleanup(\|\| { ... })` | `return () => { ... }` from `$effect` |
| Async actions | `Action::new_unsync` | Plain `async function` |
| Data loading | `LocalResource::new` | `+page.ts` `load()` or `{#await}` |
| Read-only signal | `Signal<T>` / `ReadSignal<T>` | Store `.subscribe()` or `$store` auto-sub |

### SSE Connections

**Current (Leptos, ~80 lines):**
```rust
Effect::new(move |_| {
    let es = web_sys::EventSource::new("/tama/v1/system/metrics/stream").unwrap();
    let on_snapshot = Closure::<dyn Fn(web_sys::MessageEvent)>::new(move |evt| {
        if let Some(data_str) = evt.data().as_string() {
            if let Ok(samples) = serde_json::from_str::<Vec<MetricSample>>(&data_str) {
                history.set(samples);
            }
        }
    });
    es.add_event_listener_with_callback("snapshot", on_snapshot.as_ref().unchecked_ref()).unwrap();
    on_snapshot.forget();
    on_cleanup(|| es.close());
});
```

**Proposed (Svelte, ~10 lines):**
```svelte
$effect(() => {
  const es = new EventSource('/tama/v1/system/metrics/stream');
  es.addEventListener('snapshot', (e: MessageEvent) => {
    history = JSON.parse(e.data);
  });
  es.onerror = () => { fetchFailed = true; };
  return () => es.close();
});
```

### Forms

**Current:**
```rust
<input type="text" on:input=move |ev| {
    name.set(target_value(&ev));
} />
```

**Proposed:**
```svelte
<input type="text" bind:value={name} />
```

### Conditional Rendering

**Current:**
```rust
{move || condition.get().then(|| view! { <div>...</div> })}
```

**Proposed:**
```svelte
{#if condition}
  <div>...</div>
{/if}
```

### Lists

**Current:**
```rust
{items.into_iter().map(|m| view! { <Card /> }).collect::<Vec<_>>()}
```

**Proposed:**
```svelte
{#each items as item}
  <Card {item} />
{/each}
```

## Migration Plan

### Phase 1 — Shell + Simple Pages

1. **SvelteKit scaffolding** — Initialize `ui/` directory, configure Vite, Tailwind, TypeScript
2. **Layout** — Sidebar, ToastContainer, offline banner, SSE download events
3. **Aliases** — Simple CRUD table
4. **Logs** — Read-only display with source dropdown
5. **Updates** — Check/apply with status display

### Phase 2 — Core Pages

6. **Models** — List with ModelCard components, load/unload actions
7. **Dashboard** — Metrics SSE, SparklineChart SVG components, model cards (active/inactive)
8. **Downloads** — Active downloads table + history with SSE updates

### Phase 3 — Complex Pages

9. **Model Editor** — Multi-section form (General, Sampling, Spec Decoding, Quants/Vision, Extra Args), sampling presets, pull quant wizard modal
10. **Backends** — Install/update/remove with progress, version cards
11. **Benchmarks** — Run benchmark, SSE progress, results table, history

### Phase 4 — Cleanup

12. Remove old Leptos code (`src/components/`, `src/pages/`, `src/utils/`, `src/types/`, `src/gpu.rs`, `src/jobs.rs`, `src/router.rs`, `Trunk.toml`, `index.html`, `favicon.svg`, `css/`)
13. Update `Cargo.toml` — remove `cdylib`, Leptos, wasm-bindgen, web-sys, gloo-*, wasm-bindgen-futures, futures-util, uuid, url, chrono, tempfile (CSR deps)
14. Update `router.rs` — change `include_dir!` path from `dist` to `ui/build`
15. Update `Makefile` — replace Trunk commands with pnpm
16. Visual pass — verify dark theme, spacing, responsive behavior

### What Stays Unchanged

- `tama-core/` — zero changes
- `tama-cli/` — only `include_dir!` path update in `web.rs`
- API routes (`tama-web/src/api/`) — same handlers, same endpoints
- Database schema — unchanged
- Config format — unchanged
- SSE event formats — unchanged
- OpenAPI spec generation (`utoipa`) — unchanged

## Expected Outcomes

| Metric | Before (Leptos/WASM) | After (SvelteKit) |
|--------|---------------------|-------------------|
| Initial download (uncompressed) | 5,100 KB | ~120 KB |
| Initial download (gzipped) | ~1,500 KB (if compressed) | ~35 KB |
| Frontend code | ~29,268 lines Rust | ~8,000-12,000 lines Svelte/TS |
| CSS | 18 files, ~2,431 lines | Tailwind config + ~200 lines component CSS |
| Build tool | Trunk + wasm32 target | Vite + pnpm |
| Hot reload | WASM recompile (~5-15s) | Vite HMR (near-instant) |
| SSE boilerplate per connection | ~80 lines | ~10 lines |
| Form binding | Manual helper + casting | `bind:value` |

## Trade-offs

### What You Lose

- **Compile-time type safety across the full stack.** Rust caught type mismatches at compile time. TypeScript catches many but not all across the API boundary. Mitigation: OpenAPI spec as contract, optional runtime validation.
- **Single-language codebase.** Goes from "all Rust" to "Rust + TypeScript + Svelte". Net positive for hiring (more devs know TS than Leptos).
- **Leptos fine-grained reactivity.** Svelte 5 runes are excellent but not quite as precise as Leptos signals. For Tama's CRUD + SSE workload, this is irrelevant.

### What You Gain

- **97% smaller initial load**
- **Instant hot reload** — no WASM recompilation
- **Native browser debugging** — `console.log`, breakpoints, devtools
- **Collocated CSS** — Tailwind utilities inline with markup
- **Massive ecosystem** — mature Svelte packages for any need
- **Higher AI assistant confidence** — significantly more training data for Svelte than Leptos

## Risks

| Risk | Likelihood | Mitigation |
|------|-----------|------------|
| Visual design not pixel-identical | High | Not a requirement — Tailwind will be clean. Iteration is fast. |
| SSE edge cases | Medium | Test each consumer (dashboard, downloads) against current behavior |
| CSRF token edge cases | Low | Simple port — cookie → localStorage fallback |
| Model editor state bugs | Medium | Most complex page. Thorough testing: save, rename, delete quant, pull wizard |
| CI/build breaks | Low | Simple Makefile change. CI needs `pnpm` instead of `trunk`. |

## Open Questions

1. **TypeScript strict mode?** — Recommended: `"strict": true`. Catches ~80% of what Rust would catch.
2. **Package manager?** — Default: pnpm (faster, disk-efficient, stricter). npm is fine.
3. **Old Leptos code?** — Keep in git history, delete from working tree after validation.
4. **OpenAPI codegen for TS types?** — Not needed initially. Manual interfaces are fine. Can add `openapi-typescript` codegen later if desired.

## Related Plans

- [Web UI Redesign](2026-04-04-web-ui-redesign.md) — Original dark theme implementation
- [Model Editor Redesign](2026-04-10-model-editor-redesign.md) — Current editor architecture
- [Collapsible Sidebar Navigation](2026-04-11-sidebar-navigation.md) — Current sidebar implementation
- [Move Web UI from /ui to /tama](2026-05-27-move-ui-to-tama.md) — Current URL structure

## Estimation

| Phase | Pages | Estimated Effort |
|-------|-------|-----------------|
| Phase 1 — Shell + Simple | 4 pages | 1-2 days |
| Phase 2 — Core | 3 pages | 2-3 days |
| Phase 3 — Complex | 3 pages | 2-3 days |
| Phase 4 — Cleanup | — | 0.5-1 day |
| **Total** | **10 pages** | **~5-9 days** |

---

**Last Updated**: 2026-05-31
