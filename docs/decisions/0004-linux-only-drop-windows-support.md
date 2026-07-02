# Linux-only, drop Windows support

## Context and Problem Statement

Tama originally supported both Windows and Linux, with platform-specific code for service management (Windows SCM vs. systemd), file I/O, and process supervision. Maintaining two platforms doubled test surface, added conditional compilation everywhere, and slowed development. The primary use case shifted toward Linux servers running Tama as a systemd service.

## Decision Drivers

* Reduce maintenance burden — single platform to test and support
* Simplify codebase — remove `#[cfg(target_os = "windows")]` guards
* Focus on the primary deployment target (Linux servers)
* Eliminate Windows-specific bugs (file path handling, service management, etc.)

## Considered Options

* Linux-only
* Windows + Linux (status quo)
* Cross-platform via container (Docker)

## Decision Outcome

Chosen option: "Linux-only", because the primary use case is running Tama as a systemd service on Linux. Dropping Windows removes platform abstraction layers, conditional compilation, and Windows-specific testing. The Windows installer, `windows-service` crate, and platform module were all removed.

### Consequences

* Good, because codebase is simpler — no `cfg` guards or platform modules
* Good, because testing is faster and more reliable (single platform)
* Good, because Linux-native features (systemd, inotify, signals) work without workarounds
* Bad, because Windows users cannot run Tama natively
* Bad, because existing Windows users must migrate to Linux or WSL

### Confirmation

Commit `7d98d315` removed the Windows platform module, installer, and all Windows-specific code. CI/release workflows no longer build Windows artifacts. The README documents Linux installation only (deb/rpm).

## Pros and Cons of the Options

### Linux-only

Focus on a single platform with native systemd integration.

* Good, because simpler codebase and faster development
* Good, because systemd provides robust service management
* Good, because Linux is the standard deployment target for AI servers
* Bad, because Windows users are excluded

### Windows + Linux (status quo)

Support both platforms with conditional compilation.

* Good, because broader user base
* Bad, because double the test surface and maintenance
* Bad, because platform differences cause subtle bugs
* Bad, because Windows service management is more complex than systemd

### Cross-platform via container

Run Tama in Docker on any platform.

* Good, because truly cross-platform
* Bad, because adds Docker dependency and complexity
* Bad, because GPU passthrough through containers is platform-specific anyway
