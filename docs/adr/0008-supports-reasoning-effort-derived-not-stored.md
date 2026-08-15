# `supportsReasoningEffort` is derived, not stored

Per-model reasoning-effort support exposes `supportsReasoningEffort` (boolean) and
`reasoningLevels` (array) on client-facing model info (`/v1/opencode/models`,
`/v1/models`, model detail). The database stores **only** `reasoning_levels`
(JSON TEXT column on `model_configs`); the boolean is computed at every
serialization point as "levels non-empty".

**Why:** storing both invites inconsistent states (flag off but levels present)
and doubles the migration/UI surface for no information gain — the flag carries
no information the array doesn't. The API still exposes the boolean exactly as
clients expect; it just has no column. The editor shows one text input
(comma-separated levels) instead of a checkbox + text that could disagree.

**Considered Options:**
- Stored boolean + stored array (checkbox + text input) — rejected: more
  migration surface, more UI, inconsistent states possible. The only state it
  could express that the derived form can't is "effort is supported but levels
  are unknown — client, use your defaults". We don't need that state: llama.cpp
  passes any effort string to the chat template (no defined level set), and
  vLLM 400s on values the template doesn't implement, so "unknown levels" would
  be a promise the backends can't reliably keep.
- Drop the boolean from the API entirely — rejected: clients (pi plugin) want
  an explicit capability flag independent of parsing the array.

**Consequences:**
- The opencode entry's effective `reasoning` flag = /props-computed value
  OR derived `supportsReasoningEffort` (the user-set levels also fix the
  vLLM no-`/props` default-`false` case).
- If we ever need the "effort supported, levels unknown" state, the fix is
  either a second column or a sentinel value in `reasoningLevels` — both are
  small, localized changes.
- Any new serialization point must use the `ModelConfig::supports_reasoning_effort()`
  helper rather than re-deriving `!levels.is_empty()` inline.
