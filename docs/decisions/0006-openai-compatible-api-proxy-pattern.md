# OpenAI-compatible API proxy pattern

## Context and Problem Statement

Tama manages multiple local AI backends (llama.cpp, ik_llama) running on different ports. Clients need a single endpoint to send requests to, without knowing which backend is running which model. The proxy pattern solves this by accepting OpenAI-compatible requests on a single port, routing them to the correct backend, and rewriting responses.

## Decision Drivers

* Compatibility with existing OpenAI clients (Claude Desktop, Open WebUI, etc.)
* Single port for all traffic — no client-side port management
* Automatic model-to-backend routing
* Transparent model name rewriting (user-facing names vs. backend names)

## Considered Options

* OpenAI-compatible proxy (single port, routes to backends)
* Direct backend access (each backend on its own port)
* Custom API format (not OpenAI-compatible)

## Decision Outcome

Chosen option: "OpenAI-compatible proxy", because it allows any OpenAI client to work with Tama without modification. The proxy listens on a single port (default 11435), parses the `model` field from incoming requests, looks up the correct backend, forwards the request, and streams the response back. Model names are rewritten transparently — users can set friendly names that map to backend paths.

### Consequences

* Good, because any OpenAI-compatible client works out of the box
* Good, because clients only need one endpoint URL
* Good, because the proxy handles backend lifecycle (start, stop, health check)
* Good, because model aliases and name rewriting are transparent
* Bad, because the proxy adds latency (extra hop)
* Bad, because the proxy must parse and rewrite SSE/JSON streams

### Confirmation

The proxy implements `/v1/chat/completions`, `/v1/models`, `/v1/audio/*`, and other OpenAI routes. The `forward` module handles request routing, SSE streaming, and model name rewriting. The `lifecycle` module manages backend processes. The proxy started as the core of the original KRONK spec and has evolved into the central component of Tama.

## Pros and Cons of the Options

### OpenAI-compatible proxy

Single port, routes to backends, transparent model rewriting.

* Good, because universal client compatibility
* Good, because single endpoint simplifies client configuration
* Good, because proxy can add features (auth, metrics, rate limiting) centrally
* Bad, because extra hop adds latency
* Bad, because stream parsing/rewriting is complex

### Direct backend access

Each backend on its own port, client connects directly.

* Good, because no proxy latency
* Bad, because client must know which port each model is on
* Bad, because no centralized lifecycle management
* Bad, because breaking changes in backend APIs affect clients directly

### Custom API format

Proprietary API, not OpenAI-compatible.

* Good, because full control over API design
* Bad, because no existing clients work without adapters
* Bad, because users must learn a new API
