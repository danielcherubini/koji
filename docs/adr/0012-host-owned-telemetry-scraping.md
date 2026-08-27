# Host-owned telemetry scraping: the tamad polls its own backends, the proxy never does

ADR-0010 fixed the *lifecycle* boundary (the proxy spawns nothing), but it left
telemetry ambiguous: which component knows *how* a backend is performing once it
is running? vLLM's spec-decode stats exist only on the engine's Prometheus
`/metrics` endpoint — the per-response JSON the proxy already forwards carries
nothing about speculative decoding — so the dashboard showed "spec decoding
inactive" while the engine was spec-decoding (the tamad's own log prints
"Avg Draft acceptance rate: ~45%" every 10s). Three owners were possible for
the scrape: the proxy (it holds the provider URLs), the per-request forwarder
(the payload `metrics.speculative_decoding` is experimental, n==1-only, and
silent between requests), or the host's tamad (it owns the backend lifecycle and
the port).

We decided: **the tamad scrapes**. Every managed ready backend's `/metrics` is
polled at 10s by the host daemon; the cumulative `vllm:spec_decode_*_total`
counters are diffed between scrapes (reset-tolerant) and the acceptance rate —
vLLM's own "Avg Draft acceptance rate" definition, accepted ÷ drafted
tokens — rides
the existing 1 Hz process-row stream to the proxy, which merges it into
per-server inference stats. No new protocol surface: two additive fields on the
row already traveling each second. Detection is body-driven (the counter names
in the response), so renamed vLLM installations still work and non-vLLM engines
are a cheap no-op.

**Trade-off accepted:** the scrape work runs inside the tamad's 1s stats tick;
it is bounded (2s timeout per engine, and a preflight budget check caps the
cumulative scrape work within the 3s total budget per tick) so a cluster of
stalled engines can't delay the frame past the proxy's 5s freshness gate.

**Considered Options:**
- *Proxy polls provider `/metrics`* — rejected: the proxy would start *reading*
  backend internals, the direction ADR-0010/0011 pushed the opposite way; on
  multi-host deployments it would poll across machines for data the host already
  owns.
- *Per-request `metrics.speculative_decoding` JSON* — rejected: explicitly
  experimental in vLLM, only populated for n==1, and silent between requests —
  the card would flicker back to "—/inactive" across any idle gap, which is
  exactly the bug we shipped.
- *Parse the engine's log lines* — rejected: regex on a human-oriented log,
  competing with the logs tailer for the same stream.

**Consequences:**
- Tamad gains a 10s `/metrics` scrape loop over ready backends, body-driven
  vLLM counter detection, reset-tolerant counter diffing, and two new
  `ProcessInfo` fields (`spec_accept_pct`, `spec_decoding_active`).
- The proxy stays read-only toward backend internals: it consumes two fields on
  a row it already receives at 1 Hz and merges them into per-server inference
  stats behind a 30s display freshness gate.
- Non-vLLM engines pay only the 10s scrape cost (a no-op once the body lacks the
  counters); no protocol or RPC surface was added.
