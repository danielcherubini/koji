# Three-crate Cargo workspace

Tama uses a three-crate workspace: `tama-core` (shared library), `tama` (binary), and `tama-mock` (test utilities). `tama-core` contains all business logic — proxy, backends, models, config, GPU, downloads, updates — and is the only crate with production dependencies. `tama` is a thin binary that starts the Axum server and serves the embedded SvelteKit frontend. `tama-mock` provides mock backends for integration tests.

This structure evolved from the original two-crate layout (`kronk-core` + `kronk`) when the web UI was extracted into its own crate (`tama-web`), and later collapsed back when the unified server merged the web UI into the binary crate. The CLI crate (`tama-cli`) was removed when the project moved to web-only interface (ADR-0005).

The split keeps `tama-core` as a reusable library (testable in isolation, no binary entry point) while `tama` owns server startup, feature-gated web routes, and the embedded static files. This avoids circular dependencies — the binary depends on the library, never the reverse.

**Status:** accepted

**Considered Options:**

- **Single crate** — simplest, but mixes library and binary concerns; harder to test library code without starting a server
- **Four+ crates** (separate proxy, web, CLI) — cleaner boundaries but adds build complexity and inter-crate dependency overhead for a single-user local app
- **Three crates** (chosen) — balances separation of concerns with build simplicity; `tama-core` is the clear ownership boundary for all shared logic

**Consequences:**

- Good, because `tama-core` can be tested in isolation (in-memory SQLite, mock backends) without starting HTTP servers
- Good, because the binary crate is thin — server startup, feature flags, embedded assets
- Bad, because adding new shared functionality requires careful placement (library vs binary)
- Bad, because workspace builds compile all crates even when only one changes
