# Use SvelteKit over Leptos/WASM for the frontend

## Context and Problem Statement

Tama's web UI was originally built with Leptos (Rust-based WASM framework) compiled to WebAssembly via Trunk. This meant the frontend was written in Rust, shared types with the backend, and was bundled into the same workspace. However, the WASM build pipeline added complexity — separate build steps for WASM and SSR, larger binary size, and slower development iteration.

## Decision Drivers

* Faster development iteration (hot reload, no WASM compilation)
* Smaller bundle size and faster page loads
* Ecosystem maturity (CSS modules, component libraries, npm packages)
* Simpler build pipeline (no Trunk, no WASM cross-compilation)
* Easier to iterate on UI without recompiling Rust

## Considered Options

* SvelteKit (Node.js-based, SSR + CSR hybrid)
* Leptos/WASM (Rust-based, WASM compiled)
* Pure static SPA (no SSR)

## Decision Outcome

Chosen option: "SvelteKit", because it provides a mature web development experience with hot reload, SSR for initial page load, and access to the npm ecosystem. The migration removed Leptos, Trunk, and all WASM build infrastructure, replacing them with a SvelteKit app that communicates with the backend via HTTP API calls.

### Consequences

* Good, because frontend development is fast — hot reload without recompiling Rust
* Good, because smaller JS bundle than WASM output
* Good, because access to npm ecosystem (CSS frameworks, chart libraries, etc.)
* Good, because SSR renders initial HTML server-side for fast first paint
* Bad, because frontend and backend no longer share Rust types — API contracts are implicit
* Bad, because two separate build pipelines (Rust + Node.js) instead of one

### Confirmation

The SvelteKit app is served from the Rust binary via Axum routes. SSR pages are rendered at build time and embedded. The API client in the frontend communicates with `/tama/v1/*` endpoints. All pages (Dashboard, Models, Backends, Config, Logs, Downloads, Updates, Aliases) were migrated.

## Pros and Cons of the Options

### SvelteKit

Mature Node.js framework with SSR, Svelte components, and npm ecosystem.

* Good, because hot reload makes UI iteration fast
* Good, because SSR gives fast initial page load
* Good, because Svelte has small runtime and good performance
* Good, because npm ecosystem provides rich component libraries
* Bad, because Rust and JavaScript types are not shared
* Bad, because requires Node.js in the build pipeline

### Leptos/WASM

Rust frontend compiled to WebAssembly, sharing types with backend.

* Good, because frontend and backend share Rust types (compile-time safety)
* Good, because single language for the entire stack
* Bad, because WASM compilation is slow — every UI change requires recompilation
* Bad, because WASM bundle is large (hundreds of KB)
* Bad, because limited npm ecosystem access
* Bad, because Trunk build pipeline adds complexity

### Pure static SPA

No SSR, client-side rendering only.

* Good, because simplest build pipeline
* Bad, because slow initial page load (JS must download and execute first)
* Bad, because poor SEO and accessibility without SSR

## More Information

* Commit [`c6da0fee`](https://github.com/danielcherubini/tama/commit/c6da0fee) — complete SvelteKit migration
* Commit [`86ebb4a4`](https://github.com/danielcherubini/tama/commit/86ebb4a4) — SvelteKit frontend with layout shell, API client, and stores
* [ADR-0005](./0005-web-ui-as-primary-interface-remove-cli.md) — Web UI is built with SvelteKit
