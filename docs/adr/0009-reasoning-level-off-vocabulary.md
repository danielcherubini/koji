# Reasoning level vocabulary: `off` stored, `none` on the wire

The per-model `reasoningLevels` list is stored and edited in **pi's
7-level vocabulary**: `off, minimal, low, medium, high, xhigh, max`. That is
the consumer's vocabulary — the pi client (via `pi-provider-tama`) uses those
exact strings as thinking-level keys — and "off" is the natural word in the
editor. But **no backend accepts `off` as `reasoning_effort`**: vLLM is a
strict pydantic Literal (`off` → 400) and llama.cpp special-cases only
`none` (any other string is silently passed to the chat template, where
`off` is a no-op and thinking stays ON). The ecosystem off-word is `none`
(OpenAI, OpenRouter, vLLM, llama.cpp API).

**Decision:** keep `off` in storage/UI and translate at exactly three points:
1. **pi plugin** — `thinkingLevelMap.off = "none"` (map values are the wire
   strings; pi never sends `off` itself).
2. **tama server** — the chat forwarder normalizes an incoming
   `reasoning_effort: "off"` to `"none"` before forwarding to the backend
   (safety net for clients other than pi).
3. **`reasoning_options` serializer** — the opencode-canonical field derived
   from `reasoningLevels` maps `off` → `none` (opencode's vocabulary).

**Considered Options:**
- Store `none` everywhere (opencode's word) — rejected: pi's thinking-level
  keys are fixed and include `off` but not `none`, so a stored list containing
  `none` maps to no pi level key and breaks the identity mapping the plugin
  relies on. The editor would also show a word that reads worse than "off".
- Store the wire vocabulary, convert for UI display — rejected: two sources
  of truth (display string vs stored string) with the conversion living in the
  least visible place (the form).

**Consequences:**
- The same concept has two spellings depending on where you look; the three
  translation points above are the only places where `off` and `none`
  interconvert. Any new client integration must apply the same mapping.
- The fixed validation set at the management API boundary is the pi set
  (`off, minimal, low, medium, high, xhigh, max`) — not the backend set —
  because storage speaks pi's vocabulary.
- llama.cpp builds older than 2026-07-24 ignore `reasoning_effort` entirely;
  the off mechanism on such builds would need `chat_template_kwargs:
  {"enable_thinking": false}` instead (not implemented — see research notes).
