# Parallel chunked downloads for HuggingFace models

## Context and Problem Statement

Tama downloads GGUF model files from HuggingFace, which can be several gigabytes each. The original download used the `hf-hub` crate, which downloads files sequentially with no parallelism. For large files on fast connections, this left bandwidth underutilized and made downloads slower than necessary.

## Decision Drivers

* Maximize download speed on high-bandwidth connections
* Support resume after interruption
* Respect HuggingFace rate limits and authentication
* Show accurate progress to the user

## Considered Options

* Parallel chunked downloader (custom, HTTP Range requests)
* `hf-hub` crate (status quo, sequential)
* `aria2` or `wget` subprocess

## Decision Outcome

Chosen option: "Parallel chunked downloader", because HTTP Range requests allow splitting a single file into chunks downloaded concurrently. The custom downloader calculates optimal chunk size and connection count based on file size, splits the range, downloads chunks in parallel with retry logic, and assembles the final file. It respects `HF_ENDPOINT` for custom mirrors and uses bearer token auth from the HuggingFace token.

### Consequences

* Good, because downloads are significantly faster on high-bandwidth connections
* Good, because chunks can be retried independently on failure
* Good, because progress is accurate — each chunk reports bytes downloaded
* Bad, because HTTP Range requests require server support (HuggingFace supports them)
* Bad, because adds complexity — chunk assembly, retry logic, connection pooling

### Confirmation

The downloader replaces `hf-hub` for file downloads. It uses `reqwest` with parallel tasks, calculates chunk ranges, and writes to a temporary file before renaming on completion. The download queue tracks progress per chunk and overall. PR #99 implemented the full parallel downloader with auth support.

## Pros and Cons of the Options

### Parallel chunked downloader

Custom HTTP Range-based parallel download.

* Good, because maximizes bandwidth utilization
* Good, because per-chunk retry is resilient to transient failures
* Good, because accurate progress tracking
* Good, because respects HF_ENDPOINT and auth tokens
* Bad, because more complex than a simple sequential download
* Bad, because requires Range header support from the server

### hf-hub crate (status quo)

Use the existing HuggingFace hub library.

* Good, because simple — single dependency
* Good, because handles HuggingFace API specifics
* Bad, because sequential — slow for large files
* Bad, because limited control over retry and progress

### aria2 / wget subprocess

Delegate to an external download manager.

* Good, because battle-tested download managers
* Bad, because requires external binary installed
* Bad, because harder to integrate progress tracking
* Bad, because platform-specific availability

## More Information

* PR #99: [faster HF downloads with parallel downloader](https://github.com/danielcherubini/tama/pull/99)
* Implementation plan: `docs/plans/2026-05-29-faster-hf-downloads.md`
