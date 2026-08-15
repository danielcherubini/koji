# Reasoning Effort in Model Info — Research Report

**Scope:** How to expose per-model reasoning-effort support — `supportsReasoningEffort: boolean` + `reasoningLevels: string[]` — on tama's model-info endpoints (incl. `/v1/opencode/models`), editable in the web model editor, and consumable by pi clients via the `pi-provider-tama` plugin. First target model: Qwen3.8 with levels `off / low / medium / xhigh`.

**Research date:** 2026-08 (local code + web, 5 research angles + 2 deep-dives)

---

## Executive Summary

The proposed design is viable and low-risk:

1. **Opencode safety:** opencode's model parsers (Effect `Schema.Struct`, empirically verified against the exact version opencode pins) **silently ignore unknown fields** — flat `supportsReasoningEffort`/`reasoningLevels` on `/v1/opencode/models` can never break opencode. Opencode does not consume tama's endpoint today anyway; it's an opencode-*shaped* convention, and the real consumer of the new fields is the pi plugin.
2. **Pi side is ready:** pi 0.81.0 has `Model.reasoning` + `Model.thinkingLevelMap` + `compat.supportsReasoningEffort` (+ `thinkingFormat`). The plugin currently **hardcodes `reasoning: false`** and `supportsReasoningEffort: false` — a clean upgrade. Critical gotcha: `thinkingLevelMap` holes must be explicit **`null`** (missing keys = supported), and `xhigh`/`max` only appear when explicitly mapped.
3. **The off-word problem:** no backend or ecosystem API accepts `"off"` as `reasoning_effort`. The universal off-word is **`"none"`** (OpenAI, OpenRouter, vLLM, llama.cpp). vLLM 400s on unknown values; llama.cpp silently accepts anything (so `"off"` would leave thinking *on* with no error). Resolution: keep `off` in the **UI/storage vocabulary** and translate `off → "none"` at the plugin via `thinkingLevelMap` values (pi level keys are fixed pi levels; the *values* are free-form wire strings).
4. **Tama already passes `reasoning_effort` through** to backends (body round-trips via `serde_json::Value`, no field filtering). Qwen3.8's chat template implements `low/medium/xhigh` (user-confirmed), and both backends handle `"none"` as the off switch — so the feature works end-to-end once the fields exist.

**Recommended wire shape (per model entry):**

```json
{
  "reasoning": true,
  "supportsReasoningEffort": true,
  "reasoningLevels": ["off", "low", "medium", "xhigh"]
}
```

plus optional opencode-canonical `reasoning_options: [{ "type": "effort", "values": ["none", "low", "medium", "xhigh"] }]`.

---

## Findings

### Q1 — Current `/v1/opencode/models` shape and where the new fields land (tama side)

**Route & handler** (local: `tama`)
- Route: `crates/tama-core/src/proxy/server/router.rs:87-88` → `handle_opencode_list_models` (`crates/tama-core/src/proxy/tama_handlers/models/opencode.rs:22`).
- Entry struct `ModelEntry` — `crates/tama-core/src/proxy/tama_handlers/models/utils.rs:34-51`:
  `id, name, model, backend, context_length, limit{context,output}, quant, gpu_layers, modalities (skip-if-None), tool_call, reasoning, attachment, temperature`.
- A drift-guard test (`models/tests/opencode.rs` → `test_opencode_response_deserializes_into_typed`) round-trips the wire body and pins the shape — any shape change is intentionally enforced by tests.
- Today's `reasoning: bool` is **computed per-request from backend `/props`** (`supports_preserve_reasoning`, `reasoning_format != "none"`; `opencode.rs:150-196`) — semantics: "backend preserves reasoning content". It is distinct from (and coexists with) the new user-configured `supportsReasoningEffort`.

**Storage pattern to follow** (local)
- Per-model config: `ModelConfig` — `crates/tama-core/src/config/types/model.rs:27`; complex fields (e.g. `modalities`, `vllm_config`) are **JSON-encoded TEXT columns** via `to_db_record`/`from_db_record` (model.rs:200-375).
- Migration pattern: `ALTER TABLE model_configs ADD COLUMN ... TEXT DEFAULT NULL;` with inline test — see `crates/tama-core/src/db/migrations/_0044_add_vllm_config.rs`. `ModelConfigRecord` column lists are pinned (`db/queries/types.rs:57-104`) — new columns append to the end.
- **No qwen special-casing exists anywhere** (0 code hits); a qwen model is a plain `model_configs` row. Seeding `off, low, medium, xhigh` on the Qwen3.8 model is a **data edit, not a code branch**.

**Model editor (Leptos web UI) insertion points** (local)
- Types: `ModelDetail` / `ModelForm` — `crates/tama/src/pages/model_editor/types.rs:59,174`.
- Rendering: `settings_form.rs` — plain text input precedent: `field-display-name` (settings_form.rs:151-168); init via `set_input_value` (settings_form.rs:121-135). A comma-separated "reasoning levels" input follows this pattern (split/parse in `on:input`); `gpu_layers` is an existing string-parsed-field precedent (types.rs:194).
- Save: `pages/model_editor/api.rs:140-165` builds the JSON body → `ModelBody`/`ModelPatchBody` (`crates/tama/src/api/models/crud/mod.rs:32,84`) → `apply_model_body`/`apply_model_patch` (mod.rs:192/112) → `Repository::save_model_config`.
- Read-back: GET `/tama/v1/models/:id` JSON built in `crates/tama/src/api/models/info.rs:160-210` — new fields must be added here or they won't round-trip to the editor.
- Docs: `docs/api/models.md` payload table needs the new fields.

**opencode serialization insertion points** (local)
- `ModelEntry` (utils.rs:34-51) + population in `build_model_entry` (utils.rs:60-196).
- Alias entries inherit the target's fields wholesale (opencode.rs:102-139) — new fields inherit automatically.
- `/v1/models` (OpenAI-compatible list, `proxy/handlers/models.rs:158`): alias branch (models.rs:285-305) inherits fields explicitly — extend if the fields should appear there too.

---

### Q2 — What the pi plugin does with the fields, and what pi requires

**Plugin today** (local: `~/Coding/Javascript/pi-provider-tama`)
- Fetches `GET /v1/opencode/models` (`src/tama-api.ts:4,112-139`, Bearer auth, 5s timeout).
- Entire mapping is `transformModel` (`src/tama-api.ts:141-177`):
  - `reasoning: false` **hardcoded** (tama-api.ts:162)
  - `compat: { ...DEFAULT_COMPAT, ...BACKEND_COMPAT }` with `supportsReasoningEffort: false` **hardcoded** (tama-api.ts:21-24)
  - `thinkingLevelMap` — not set at all
  - Unknown opencode fields are **dropped** (explicit `TamaModel` type, `src/types.ts:2-18`)
- Tests pin current behavior (`test/tama-api.test.ts:71,78,91,102,111,170-172`) — must update.
- Both model paths (startup + refresh) go through `transformModel` (`src/index.ts:95-106`), so one change covers all.

**Pi's `Model` type** (local: `node_modules/@earendil-works/pi-ai@0.81.0/dist/types.d.ts`)
- `Model` (types.d.ts:596-627): `reasoning: boolean`, `thinkingLevelMap?: ThinkingLevelMap`, `input: ("text"|"image")[]`, `compat?: OpenAICompletionsCompat`.
- `ThinkingLevel = "minimal"|"low"|"medium"|"high"|"xhigh"|"max"`; `ModelThinkingLevel = "off" | ThinkingLevel`; `ThinkingLevelMap = Partial<Record<ModelThinkingLevel, string | null>>` (types.d.ts:22-24).
- `OpenAICompletionsCompat` (types.d.ts:405-465): `supportsReasoningEffort?: boolean` (gates top-level `reasoning_effort` in the request), `thinkingFormat?: "openai"|"openrouter"|"deepseek"|"together"|"zai"|"qwen"|"chat-template"|"qwen-chat-template"|"string-thinking"|"ant-ling"`, `chatTemplateKwargs?`.

**`thinkingLevelMap` semantics — the critical gotcha** (pi-ai `dist/models.js:391-420`)
- For a `reasoning: true` model, **missing keys are *supported*** via provider defaults (`off`…`high` appear in the selector). Hidden levels must be explicit **`null`**.
- `xhigh` and `max` are **opt-in** — appear only with an explicit non-null entry.
- `off: null` removes "off" from the selector entirely (thinking can't be disabled).
- `clampThinkingLevel` rounds **up** first, then down.
- `reasoning: false` short-circuits everything: selector = `["off"]`, no thinking params ever sent.

**Request-time behavior, default `"openai"` thinkingFormat** (pi-ai `dist/api/openai-completions.js`)
- Chosen level → `reasoning_effort: thinkingLevelMap[level] ?? level` (line ~570-574).
- Off: `off` mapped to a **string** → sends `reasoning_effort: "<string>"`; `off` **omitted** → sends nothing; `off: null` → user can't pick off.
- `supportsReasoningEffort: false` → `reasoning_effort` **never** sent.
- `thinkingFormat: "qwen"` / `"qwen-chat-template"` are **boolean-only** (`enable_thinking` on/off; level value dropped) and exist for talking *directly* to Qwen backends — **not** the right format for the pi→tama hop (tama is an OpenAI-compatible proxy that should receive `reasoning_effort` and translate per backend).
- Compat defaults for unknown providers auto-detect `supportsReasoningEffort: true`, but the plugin's explicit `model.compat` wins per-field — so today's hardcoded `false` is what's in effect.

**UI consumption** (pi-coding-agent `dist/core/agent-session.js:1276-1324`, `dist/modes/interactive/components/thinking-selector.js`)
- The thinking selector renders exactly `getSupportedThinkingLevels(model)`; nulls hidden; footer thinking indicator only when `model.reasoning`.
- Default level `medium` (`dist/core/defaults.js:1`), clamped to available levels on session restore.

**Plugin changes required**
- `src/types.ts` — add `supportsReasoningEffort?: boolean`, `reasoningLevels?: string[]` to `TamaModel`; `thinkingLevelMap` to `PiModel`.
- `src/tama-api.ts` `transformModel` — replace hardcoded `reasoning: false` / `supportsReasoningEffort: false`; build `thinkingLevelMap` with explicit `null` holes; keep the existing `BACKEND_COMPAT` merge intact (maxTokensField etc.).
- `test/tama-api.test.ts` — update pinned assertions.

**Recommended mapping**

| tama wire field | pi `Model` field |
|---|---|
| `supportsReasoningEffort: true` | `reasoning: true` + `compat.supportsReasoningEffort: true` |
| `reasoningLevels: ["off","low","medium","xhigh"]` | `thinkingLevelMap: { off: "none", minimal: null, low: "low", medium: "medium", high: null, xhigh: "xhigh", max: null }` |
| (none) | `compat.thinkingFormat` left at default `"openai"` |

The `off: "none"` value is the off-word translation (see Q4). If a model could emit values outside pi's 7 levels, the plugin must normalize (unknown strings are sent verbatim as `reasoning_effort`).

---

### Q3 — Opencode's official model structure (web)

**Shapes** (opencode `dev` @ commit `4643e65`, 2026-08-14)
- Three authoritative shapes, all Effect `Schema.Struct`:
  - **models.dev catalog `Model`** — `packages/core/src/models-dev.ts:52-88`; fetched from `https://models.opencode.ai/api.json`. This flat shape is what tama's entries mirror. It already contains the canonical equivalent of this feature:
    ```
    reasoning_options?: Array<
      { type: "effort", values: (string|null)[] } |
      { type: "toggle" } |
      { type: "budget_tokens", min?, max? }
    >
    ```
    Live catalog: 2,347 models use `type:"effort"`; observed value set: `none, minimal, low, medium, high, xhigh, max`. **`"off"` is not in opencode's vocabulary — its off-word is `"none"`.**
  - **Provider API `Model`** (served on opencode's `GET /provider`) — `packages/opencode/src/provider/provider.ts:959-1052`: `capabilities{reasoning, attachment, toolcall, input/output modalities, ...}`, `cost`, `limit`, `options: Record<string, any>` (generic escape hatch), `variants: Record<string, Record<string, any>>` (where effort levels materialize).
  - **`ModelV2.Info`** (internal) — `packages/schema/src/model.ts` + spec `specs/v2/provider-model.md`.
- Legacy zod-era shape (commit `5c5069b6`, `packages/opencode/src/provider/models.ts`) is the exact flat field set tama emits today.

**Reasoning in opencode**
- `reasoningVariants()` (`packages/opencode/src/provider/transform.ts:1656`) converts catalog `reasoning_options` → per-model `variants`; any string value becomes a variant (unknown strings tolerated; `null` → `"none"`).
- Wire: `reasoningEffort()` (transform.ts:1721) → for `@ai-sdk/openai-compatible`: `{ reasoningEffort }` → **top-level `reasoning_effort`** in the chat body (confirmed in `@ai-sdk/openai-compatible@2.0.41`).
- UI: variant picker dialog + `ctrl+t` cycle; docs at `opencode.ai/docs/models#variants`.

**Strictness — extra fields are safe** (verified empirically, `effect@4.0.0-beta.83`, the exact pinned version)
- `Schema.Struct` decode and `Schema.is` **ignore extra fields**; missing required fields are rejected. Adding `supportsReasoningEffort`/`reasoningLevels` **can never break any current opencode parser**.
- Opencode has no field by those names anywhere; generic passthroughs are `options` (Record) and `variants`.

**Does opencode consume tama's endpoint?**
- No. `/v1/opencode/models` is tama's own convention. Opencode gets model lists from the models.dev catalog + `opencode.json` `provider.<id>.models`. Auto-detection of a provider's `/models` endpoint exists as merged-but-unshipped work (PR #8359 → `models-endpoint` branch; issue #6231 open) and would read the **plain OpenAI `{data:[{id}]}` list** anyway.
- Implication: the endpoint is opencode-*shaped* for ecosystem familiarity; the new fields are strictly for non-opencode clients (pi) — harmless extras for opencode.

**Optional canonicality bonus:** also emit `reasoning_options: [{ "type": "effort", "values": ["none","low","medium","xhigh"] }]` (converting `off`→`none`) so a future opencode client picks the levels up natively.

---

### Q4 — Chat path + backend acceptance of `reasoning_effort` (deep-dive)

**Tama forwards it already** (local)
- `handle_chat_completions` (`proxy/handlers/chat.rs:104`) parses the body as `serde_json::Value` **only** to read `model`, then forwards the original bytes.
- `forward_request` (`proxy/forward/request.rs:19`) re-parses to `Value` and makes **only two mutations**: `model` rewrite (request.rs:216-220) and langfuse `stream_options.include_usage` (request.rs:221-232). **No strict struct, no whitelist, no field dropping** — `reasoning_effort` survives verbatim today.
- Response side: `reasoning_content` passes through unchanged in both stream and non-stream (only `model` rewritten; `proxy/forward/json.rs:3-9`, `proxy/forward/sse.rs:13-47`).
- **Injection point** for any server-side translation: `request.rs:214-235` — the resolved `ModelConfig` is already read at request.rs:159-163. (The `remote:` provider branch, chat.rs:63-90, forwards raw bytes and bypasses this.)
- Per-request param injection is a **new pattern** (existing `SamplingParams` are launch-time CLI args: `profiles.rs:109`, `config/resolve/mod.rs`); no `reasoning_effort`/`enable_thinking`/`chat_template_kwargs` references exist anywhere in the repo — greenfield.

**Backend acceptance** (web; llama.cpp @ `6b4344ecc7e6` 2026-08-14, vLLM main @ `44fc57d7b72e` 2026-08-15)

| Backend | Field | Accepted values | Off mechanism | Unknown values |
|---|---|---|---|---|
| llama.cpp `/v1/chat/completions` | `reasoning_effort` (top-level string) | Only `"none"` special-cased (`enable_thinking=false`, PR #26045, 2026-07-24); since commit `7e4c0a96` (2026-08-14) other values pass into the jinja chat template | `reasoning_effort:"none"`; or `chat_template_kwargs:{"enable_thinking":false}` (boolean required, else 400) | **Silently accepted** (no 400) — `"off"` would leave thinking ON |
| vLLM `/v1/chat/completions` | `reasoning_effort` (strict pydantic Literal) | `none, minimal, low, medium, high, xhigh, max` | `"none"` → `enable_thinking=false` + `include_reasoning=false`; or `chat_template_kwargs:{"enable_thinking":false}` | **400 validation error** (e.g. `"off"`) |
| OpenAI (reference) | `reasoning_effort` | `none, minimal, low, medium, high, xhigh, max` (model-dependent) | `"none"` | 400 |
| OpenRouter (reference) | `reasoning_effort` | `minimal…xhigh, max, none` | `"none"` | — |

Key backend facts:
- **No backend accepts `"off"`.** Ecosystem off-word is `"none"` (`off` appears only in llama.cpp's *CLI* `--reasoning off`).
- **Qwen3.8 supports `low/medium/xhigh`** (user-confirmed) — the levels reach the model via the chat-template pass-through (llama.cpp commit `7e4c0a96`) / vLLM template injection. (llama.cpp's caps-test finding that the *classic Qwen3* template doesn't implement effort applies to older templates only.)
- **Version sensitivity:** llama.cpp builds before 2026-07-24 ignore `reasoning_effort` entirely (discussion #20408); vLLM enum grew over time (`none` PR #36238 2026-03-11; `max` PR #40982 2026-04-29) — older deployments 400 on newer values.
- vLLM `"none"` has known parser-specific bugs (#37909 empty think blocks, #39581 Nemotron-H); some tokenizers reject injected kwargs (#38560).
- llama-cpp-python: per-request `reasoning_effort` unreleased (PR #2167 open); per-request `chat_template_kwargs` unsupported (issue #2063).

**Resulting design decisions (gap resolutions)**
- Storage/UI vocabulary: `off, low, medium, xhigh` (what the user types).
- Wire vocabulary to backends: `none, low, medium, high, xhigh, max` — translate `off → "none"` at the **plugin** via `thinkingLevelMap` values; optionally also normalize `off → "none"` in tama at `request.rs:214-235` as a safety net for other clients.
- Validate editor input against the known set (`off, minimal, low, medium, high, xhigh, max`) since vLLM 400s on unknowns.

---

### Q5 — Recommended end-to-end shape

Per-model entry in `/v1/opencode/models` (and the tama model detail API):

```json
{
  "id": "...", "name": "...",
  "reasoning": true,
  "supportsReasoningEffort": true,
  "reasoningLevels": ["off", "low", "medium", "xhigh"],
  "reasoning_options": [{ "type": "effort", "values": ["none", "low", "medium", "xhigh"] }]
}
```

- `reasoning` (existing, computed from /props) = backend preserves reasoning content.
- `supportsReasoningEffort` = user-configured: effort levels are adjustable.
- `reasoningLevels` = UI vocabulary (pi level keys); drives both the editor display and the pi plugin's `thinkingLevelMap`.
- `reasoning_options` (optional) = opencode-canonical, off mapped to `none`.
- Editor: comma-separated text input `off, low, medium, xhigh` → parsed/validated to the array; `supportsReasoningEffort` = checkbox (or derived: `reasoningLevels.length > 0`).
- Plugin: `reasoning: supportsReasoningEffort`, `compat.supportsReasoningEffort: supportsReasoningEffort`, `thinkingLevelMap` with `off: "none"` and explicit `null` holes for the 5 unlisted pi levels.
- Chat path: no change required (passthrough already works); optional `off→none` normalization.

---

## Evidence & Credibility

| Source | Type / credibility |
|---|---|
| `tama` repo: `proxy/server/router.rs`, `proxy/tama_handlers/models/{opencode.rs,utils.rs,tests/opencode.rs}`, `config/types/model.rs`, `db/queries/types.rs`, `db/migrations/_0044_add_vllm_config.rs`, `src/pages/model_editor/*`, `src/api/models/{crud/info}`, `proxy/handlers/chat.rs`, `proxy/forward/request.rs`, `profiles.rs`, `config/resolve/mod.rs` | Official source code (1st tier) — all file:line verified in-repo |
| `pi-provider-tama` repo: `src/{index.ts,tama-api.ts,types.ts}`, `test/tama-api.test.ts` | Official source code (1st tier) |
| `@earendil-works/pi-ai@0.81.0` `dist/types.d.ts`, `dist/models.js`, `dist/api/openai-completions.js`, `dist/providers/data/*.json`; `@earendil-works/pi-coding-agent@0.81.0` `dist/core/*`, `dist/modes/interactive/components/*`; pi docs `models.md`, `custom-provider.md` | Official source/docs of the consuming agent (1st tier). Note minor docs-vs-0.81.0 drift (`baseten` thinkingFormat in docs only) |
| opencode `dev` @ `4643e65`: `packages/core/src/models-dev.ts`, `packages/opencode/src/provider/{provider.ts,transform.ts}`, `packages/schema/src/model.ts`, `specs/v2/provider-model.md`, TUI variant dialog/keybinds | Official source (1st tier), read from cloned repo |
| `https://models.opencode.ai/api.json` (live, 6,583 models) | Official live data (1st tier) — vocab statistics computed over full catalog |
| `opencode.ai/docs/{models,providers}`, changelog | Official docs (1st tier) |
| llama.cpp: `tools/server/server-common.cpp` (`oaicompat_chat_params_parse`), `tools/server/README.md`, PRs #26045/#13196/#13771/#23116/#21250, commits `7e4c0a96`/`c31c9bc`/`27209a59`, discussion #20408, issues #13160/#20182; llama-cpp-python PR #2167/#2168, issue #2063 | Official source/PRs (1st tier) |
| vLLM: `vllm/entrypoints/openai/chat_completion/protocol.py`, PRs #36238/#40982/#43401/#50580, issues #37909/#39581/#38560, docs.vllm.ai reasoning_outputs, qwen.readthedocs.io deployment/vllm | Official source/docs (1st tier) |
| OpenAI `developers.openai.com/api/docs/guides/reasoning`, openai-python `reasoning_effort.py`; OpenRouter `openrouter.ai/docs/api_reference/parameters` | Official docs/SDK types (1st tier) |
| Effect `Schema.Struct` extra-field behavior — tested locally against `effect@4.0.0-beta.83` (opencode's pinned version) | Empirical verification (1st tier) |
| Qwen3.8 supports low/medium/xhigh effort levels | **User statement (maintainer domain knowledge)** — not independently source-verified; consistent with llama.cpp template pass-through mechanism |

**Credibility notes:** opencode `reasoning_options` is a dev-branch feature (shipped ~v1.18.x, July 2026 per changelog; present in the live catalog) — released stable versions may lag. vLLM release *tags* for enum changes were verified by commit/PR, not by tag. llama.cpp effort support is very recent (2026-07/08) and version-sensitive.

---

## Unresolved Contradictions / Tensions

1. **`off` vs `none`** — resolved for the flat fields (keep `off` in UI/storage since pi is the consumer; translate at plugin via `thinkingLevelMap` values; use `none` if emitting opencode `reasoning_options`). The split is by design, not a bug.
2. **`reasoning: true` without effort control** — a model that thinks but has no adjustable levels would map to pi `reasoning: true` + `compat.supportsReasoningEffort: false`; pi would still show the full selector (all levels "supported" by default) while sending no `reasoning_effort`. The plugin would need an all-null (or minimal) `thinkingLevelMap` to hide levels in that case. Not needed for the first Qwen3.8 model (which has levels).
3. **`reasoning` (opencode/capability) vs `supportsReasoningEffort` (effort control)** — different semantics that both appear per model; document clearly so they aren't conflated (opencode's `reasoning` = "produces reasoning content"; the new bool = "effort is adjustable").

---

## Gaps / What Remains Unknown

- **Tama's pinned llama.cpp build** — is it ≥ 2026-07-24 (so `reasoning_effort` is honored)? If tama ships/pins an older build, `none`-based off is broken on llama.cpp until the bump (vLLM unaffected).
- **Qwen3.8 template details** — exact level set per template version (user says low/medium/xhigh exist; whether the template distinguishes `medium` vs `high`, or treats unknown levels, is template-dependent and unverified).
- **Scope decisions (design, not research):** whether `supportsReasoningEffort` is `bool` vs `Option<bool>`; whether the editor validates levels against the fixed set or accepts arbitrary strings; whether to also expose the fields on `/v1/models` (OpenAI list) and on the `tama/v1` detail API; whether `reasoning_options` is emitted in v1 or deferred.
- **Edge:** pi `reasoning: true` + no levels (see tensions #2) — decide plugin behavior before other thinking-models without effort support are added.
