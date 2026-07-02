# Web UI as primary interface, remove CLI

## Context and Problem Statement

Tama had two interfaces: a CLI (`tama-cli`) for management commands and a web UI (`tama-web`) for browser-based control. Both were separate crates in the workspace. The CLI was useful for scripting but required users to learn command syntax, while the web UI provided a visual interface. Maintaining two interfaces meant duplicating logic and keeping both in sync.

## Decision Drivers

* Single interface reduces maintenance burden
* Web UI is more discoverable — users can see all options visually
* The binary itself can serve the web UI (zero-arg startup)
* CLI commands were mostly convenience wrappers around the same API the web UI uses

## Considered Options

* Web UI only (binary serves HTTP, zero-arg startup)
* CLI + web UI (status quo)
* CLI only, remove web UI

## Decision Outcome

Chosen option: "Web UI only", because the web UI covers all management functionality and is more accessible to users. The `tama-web` crate was renamed to `tama` with a `[[bin]]` target, and `tama-cli` was removed entirely. The binary starts the HTTP server with no arguments and serves the web UI at the root path.

### Consequences

* Good, because single binary — no separate CLI to install or maintain
* Good, because web UI is more discoverable than CLI commands
* Good, because all state management goes through one code path
* Bad, because scripting/automation requires HTTP API calls instead of CLI commands
* Bad, because no `man` pages or shell completion

### Confirmation

PR #135 removed `tama-cli`, renamed `tama-web` to `tama`, and added a `main.rs` with zero-arg server startup. The `ssr` feature gates the binary target so it is not compiled for WASM. The Makefile and CI were updated for the new layout.

## Pros and Cons of the Options

### Web UI only

Single binary that serves an HTTP server and embedded web UI.

* Good, because simplest user experience — run `tama` and open a browser
* Good, because all functionality is in one place
* Good, because no CLI to maintain in sync with the API
* Bad, because no native CLI for scripting
* Bad, because requires a browser (not suitable for headless servers without VNC/SSH tunneling)

### CLI + web UI (status quo)

Two separate binaries, each with their own interface.

* Good, because CLI is scriptable and works headless
* Good, because web UI is visual and discoverable
* Bad, because logic is duplicated between CLI and web handlers
* Bad, because two crates to maintain and test

### CLI only

Remove web UI, keep CLI as the sole interface.

* Good, because lightweight and scriptable
* Bad, because no visual interface for complex operations
* Bad, because users must memorize command syntax

## More Information

* PR #135: [remove CLI, promote web UI to binary](https://github.com/danielcherubini/tama/pull/135)
* Implementation plan: `docs/plans/2026-07-01-remove-cli-promote-web.md`
