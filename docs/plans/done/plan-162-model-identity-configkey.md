# Model Identity `ConfigKey` Plan

**Goal:** Introduce a `ConfigKey` newtype in tama-core with `ConfigKey::from_repo_id()` as the only derivation site for the `config_key = repo_id.to_lowercase().replace('/', "--")` rule, replace all open-coded sites, give the case-preserving card-filename slug its own named function, and disambiguate the two same-named `resolve_model_id` functions.

**Architecture:** Audit finding F5. A model has 5+ identifier forms (DB i64 id, config_key, repo_id, api_name, alias) with no type distinguishing them. The derivation rule is open-coded at 14+ sites and applied inconsistently: the pull paths (`crates/tama-core/src/proxy/tama_handlers/pull/handlers.rs:133,256,386`) skip the lowercase step, so display-name lookups into the lowercased registry silently miss for mixed-case repo ids (live bug — `_setup_model_after_pull_with_config` inserts with `repo_slug.to_lowercase()` at `pull/verify.rs:~292`), and `pull_quant_wizard.rs:451` reverses the call order. Model CARD filenames (`<configs_dir>/<slug>.toml`) deliberately use a DIFFERENT, case-preserving rule (`proxy/state.rs:202-207`) — cards already exist on disk with mixed-case names, so that rule must NOT be lowercased; it gets its own named function instead. Two functions named `resolve_model_id` have different semantics (config-key resolution returning `String` vs DB-id resolution returning `Result<Option<i64>>`); they are renamed to `resolve_config_key` / `resolve_db_id`.

**Tech Stack:** Rust, serde

---

### Task 1: Introduce the `ConfigKey` newtype in tama-core

**Context:**
No type distinguishes a config_key from a repo_id or api_name today — everything is `String`. Decision: `ConfigKey` lives in `crates/tama-core/src/models/` (NOT `config/types/`) because its inverse `config_key_to_repo_id` already lives in `crates/tama-core/src/models/mod.rs:27` and model identity is a models-domain concern. The forward rule lives in `ConfigKey::from_repo_id`; the inverse rule MOVES into `ConfigKey::to_repo_id`, and the existing free fn `config_key_to_repo_id` becomes a one-line delegate (do NOT delete it — it has ~15 callers across both crates). `ConfigKey` is intentionally NOT wired into manager/Repository/handler signatures in this plan — it is adopted at derivation points only; signature migration would be a follow-up.

**Files:**
- Create: `crates/tama-core/src/models/config_key.rs`
- Modify: `crates/tama-core/src/models/mod.rs`

**What to implement:**

1. **`crates/tama-core/src/models/config_key.rs`**:
   ```rust
   //! Typed model identity: the `ConfigKey` newtype.
   //!
   //! A model's registry/lookup key is derived from its HuggingFace `repo_id`.
   //! The derivation rule lives ONLY in `ConfigKey::from_repo_id` — never
   //! re-derive it inline. Model CARD filenames use a different,
   //! case-preserving rule; see `crate::models::card_slug` (Task 4).

   use serde::{Deserialize, Serialize};
   use std::fmt;
   use std::str::FromStr;

   /// Registry key for a model config (e.g. `unsloth--gemma-4-26b-a4b-it-gguf`).
   ///
   /// Invariant: produced by `ConfigKey::from_repo_id` (or trusted verbatim via
   /// `new`/`FromStr` when read from the DB, a URL, or the registry map).
   #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
   #[serde(transparent)]
   pub struct ConfigKey(String);

   impl ConfigKey {
       /// Derive the config key for a repo id.
       ///
       /// THE ONLY derivation site for the rule:
       /// `config_key = repo_id.to_lowercase().replace('/', "--")`.
       pub fn from_repo_id(repo_id: &str) -> Self {
           Self(repo_id.to_lowercase().replace('/', "--"))
       }

       /// Wrap a string that is already a config key (read from the DB, a URL
       /// path segment, or the registry map key). Does NOT transform the input.
       pub fn new(key: impl Into<String>) -> Self {
           Self(key.into())
       }

       /// The key as a string slice.
       pub fn as_str(&self) -> &str {
           &self.0
       }

       /// Convert back to the repo_id stored in the DB (e.g.
       /// `unsloth--gemma-4-26b-a4b-it-gguf` → `unsloth/gemma-4-26b-a4b-it-gguf`).
       ///
       /// Inverse of `from_repo_id` up to case (repo_id lookups are
       /// case-insensitive via `COLLATE NOCASE` on `model_configs.repo_id`).
       /// Only the FIRST `--` is split — repo ids have exactly one path segment.
       pub fn to_repo_id(&self) -> String {
           if let Some(idx) = self.0.find("--") {
               let (prefix, suffix) = self.0.split_at(idx);
               format!("{}/{}", prefix, &suffix[2..])
           } else {
               self.0.clone()
           }
       }
   }

   impl fmt::Display for ConfigKey {
       fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
           f.write_str(&self.0)
       }
   }

   impl AsRef<str> for ConfigKey {
       fn as_ref(&self) -> &str {
           &self.0
       }
   }

   impl FromStr for ConfigKey {
       type Err = std::convert::Infallible;
       /// Wraps the input VERBATIM (assumes it is already a config key).
       fn from_str(s: &str) -> Result<Self, Self::Err> {
           Ok(Self(s.to_string()))
       }
   }
   ```

2. **`crates/tama-core/src/models/mod.rs`**: add `pub mod config_key;` (alphabetical: after `pub mod card;`) and `pub use config_key::ConfigKey;` (after the `pub use card::...` line). Then change `config_key_to_repo_id` (lines 27–34) to delegate:
   ```rust
   pub fn config_key_to_repo_id(config_key: &str) -> String {
       ConfigKey::new(config_key).to_repo_id()
   }
   ```
   Keep its doc comment (update it to mention `ConfigKey::to_repo_id` as the canonical home).

3. **Tests** in `crates/tama-core/src/models/config_key.rs` (`#[cfg(test)] mod tests`):
   - `test_from_repo_id_derives_canonical_key`: `"Unsloth/Gemma-4-26B-A4B-IT-GGUF"` → `"unsloth--gemma-4-26b-a4b-it-gguf"`; `"owner/repo"` → `"owner--repo"`.
   - `test_from_repo_id_lowercases_and_replaces`: mixed case + slash both handled; already-lowercase id unchanged.
   - `test_from_repo_id_without_slash`: `"local-model"` → `"local-model"` (lowercased only).
   - `test_to_repo_id_inverts_first_double_dash`: `"owner--repo"` → `"owner/repo"`; `"local-model"` → `"local-model"` (no `--` → unchanged).
   - `test_round_trip`: `ConfigKey::from_repo_id("Owner/Repo").to_repo_id()` → `"owner/repo"` (case loss is expected — assert it explicitly with a comment pointing at the `COLLATE NOCASE` contract).
   - `test_new_and_from_str_wrap_verbatim`: `ConfigKey::new("Owner--Repo").as_str()` == `"Owner--Repo"` (no transformation); same for `"x".parse::<ConfigKey>()`.
   - `test_display_and_as_ref`: `format!("{}", key)` and `&key as &str` via `AsRef`.
   - `test_serde_transparent`: `serde_json::to_string(&ConfigKey::from_repo_id("a/b"))` == `"\"a--b\""` and deserialization round-trips.

**Steps:**
- [ ] Write the failing tests in `crates/tama-core/src/models/config_key.rs` (file won't compile until the struct exists — expected)
- [ ] Run `cargo nextest run --package tama-core -- models::config_key` — verify failure
- [ ] Implement `ConfigKey` + module wiring + `config_key_to_repo_id` delegation
- [ ] Run `cargo nextest run --package tama-core -- models::config_key` — 8 tests pass
- [ ] Run `cargo nextest run --package tama-core` — all pass (the delegation touches `config_key_to_repo_id`, which has existing callers/tests)
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "feat: add ConfigKey newtype as the single config_key derivation site"

**Acceptance criteria:**
- [ ] `tama_core::models::ConfigKey` exists with `from_repo_id`/`new`/`as_str`/`to_repo_id`/`Display`/`FromStr`/serde-transparent
- [ ] `config_key_to_repo_id` delegates to `ConfigKey::to_repo_id` — inverse rule has exactly one implementation
- [ ] 8 new unit tests pass; whole-crate suite green

---

### Task 2: Replace open-coded derivation sites in tama-core

**Context:**
With `ConfigKey` available, every inline `repo_id.to_lowercase().replace('/', "--")` (and the pull paths' missing-lowercase variant) in tama-core moves to `ConfigKey::from_repo_id`. Verified sites: `crates/tama-core/src/db/mod.rs:35`, `crates/tama-core/src/db/repository.rs:287`, `crates/tama-core/src/models/update.rs:271`, `crates/tama-core/src/db/backfill/initial_backfill.rs:55`, `crates/tama-core/src/models/verify.rs:300` (test), `crates/tama-core/src/proxy/tama_handlers/models/utils.rs:57`, `crates/tama-core/src/proxy/tama_handlers/pull/handlers.rs:133,256,386`, `crates/tama-core/src/proxy/tama_handlers/tests.rs:111,185` (tests). Decision on the pull/handlers.rs sites: they look up `state.model_configs` with `format!("{}--{}", repo_id.replace('/', "--"), quant)` — replace ONLY the derivation part with `ConfigKey::from_repo_id(&repo_id)` (this also FIXES the mixed-case miss — intended); do NOT touch the `--{quant}` suffix semantics of those lookup keys (whether quant-suffixed keys can match is a separate question, out of scope). Card-filename sites are Task 4, NOT this task — do not touch `pull/verify.rs:213` or `state.rs:206` here.

**Files:**
- Modify: `crates/tama-core/src/db/mod.rs`
- Modify: `crates/tama-core/src/db/repository.rs`
- Modify: `crates/tama-core/src/models/update.rs`
- Modify: `crates/tama-core/src/db/backfill/initial_backfill.rs`
- Modify: `crates/tama-core/src/models/verify.rs`
- Modify: `crates/tama-core/src/proxy/tama_handlers/models/utils.rs`
- Modify: `crates/tama-core/src/proxy/tama_handlers/pull/handlers.rs`
- Modify: `crates/tama-core/src/proxy/tama_handlers/tests.rs`

**What to implement:**

1. Per site, replace the inline expression with `ConfigKey::from_repo_id(<repo_id expr>)` and use `.as_str()` / `.to_string()` / `&key` (Display) at the consumption point:
   - `db/mod.rs:35` — `let config_key = record.repo_id.to_lowercase().replace('/', "--");` → `let config_key = crate::models::ConfigKey::from_repo_id(&record.repo_id).to_string();` (the map key type is `HashMap<String, ModelConfig>` — keep it `String`; ConfigKey adoption in the registry map type is a follow-up). Also update the doc comment at lines 25–26 to point at `ConfigKey::from_repo_id`.
   - `db/repository.rs:287` — same replacement inside `load_model_configs`; update the doc comment at line 280.
   - `models/update.rs:271` and `db/backfill/initial_backfill.rs:55` — both feed `crate::db::save_model_config(conn, &config_key, &mc)` (takes `&str`) → `let config_key = crate::models::ConfigKey::from_repo_id(repo_id);` then pass `config_key.as_str()`.
   - `models/verify.rs:300` (test) — same pattern via `mgr.save_model_config(config_key.as_str(), &mc)`.
   - `tama_handlers/models/utils.rs:57` — inside `resolve_model_id` (renamed in Task 5): `return raw.to_lowercase().replace('/', "--");` → `return ConfigKey::from_repo_id(raw).to_string();` with `use crate::models::ConfigKey;` at the top.
   - `tama_handlers/pull/handlers.rs:133,256,386` — `repo_id.replace('/', "--")` inside `format!("{}--{}", ..., quant…)` → `ConfigKey::from_repo_id(&repo_id)` (ConfigKey implements Display, so it drops into the format string directly). This intentionally fixes the mixed-case miss.
   - `tama_handlers/tests.rs:111,185` — `repo_id.replace('/', "--").to_lowercase()` → `ConfigKey::from_repo_id(repo_id).to_string()`.
2. After edits: `rg 'to_lowercase\(\)\.replace|replace\([^)]*\)\.to_lowercase' crates/tama-core/src` must return zero hits outside `models/config_key.rs` and card-slug tests (Task 4 territory: `tama_handlers/tests.rs:16` is a CARD path — leave it for Task 4).

**Steps:**
- [ ] Run `cargo nextest run --package tama-core` — green baseline
- [ ] Apply the replacements per site above
- [ ] Run the rg check above — zero hits outside `config_key.rs`
- [ ] Run `cargo nextest run --package tama-core` — all pass (pull tests and tama_handlers tests exercise these paths)
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "refactor: route all tama-core config_key derivations through ConfigKey"

**Acceptance criteria:**
- [ ] No inline `to_lowercase().replace('/', "--")` (or reversed/missing-lowercase variants) remains in tama-core outside `models/config_key.rs`
- [ ] pull/handlers.rs display-name lookups derive via `ConfigKey::from_repo_id` (mixed-case bug fixed); quant-suffix lookup semantics otherwise unchanged
- [ ] `cargo nextest run --package tama-core` passes; clippy clean

---

### Task 3: Replace open-coded derivation sites in the `tama` crate (incl. WASM-safe helper)

**Context:**
Remaining config_key derivations live in the `tama` crate: `crates/tama/src/api/models/crud/update.rs:88,171`, `crud/rename.rs:106`, `crud/delete.rs:89` (all server-side, can use `tama_core::models::ConfigKey`), and `crates/tama/src/components/pull_quant_wizard.rs:451` — a Leptos component compiled for BOTH csr (WASM) and ssr. The `tama` crate only depends on `tama-core` under the `ssr` feature (`crates/tama/Cargo.toml`: `tama-core = { path = "../tama-core", optional = true }`), so the wizard CANNOT reference `ConfigKey` in csr builds. Decision: server-side handlers use `tama_core::models::ConfigKey` directly; the wasm-compilable mirror goes in `crates/tama/src/utils/mod.rs` (no tama-core dependency, compiled for both targets) as a documented one-line mirror — this is deliberate duplication for the WASM boundary, not drift.

**Files:**
- Modify: `crates/tama/src/api/models/crud/update.rs`
- Modify: `crates/tama/src/api/models/crud/rename.rs`
- Modify: `crates/tama/src/api/models/crud/delete.rs`
- Modify: `crates/tama/src/utils/mod.rs`
- Modify: `crates/tama/src/components/pull_quant_wizard.rs`

**What to implement:**

1. `crud/update.rs:88` and `:171` — `let config_key = existing_record.repo_id.to_lowercase().replace('/', "--");` → `let config_key = tama_core::models::ConfigKey::from_repo_id(&existing_record.repo_id);` and pass `config_key.as_str()` to `repo.save_model_config(...)` (post-plan-160 signature is `save_model_config(&self, config_key: &str, mc: &ModelConfig)`).
2. `crud/rename.rs:106` — same replacement for `new_repo_id`.
3. `crud/delete.rs:89` — same replacement for `repo_id` (in `delete_quant`).
4. `crates/tama/src/utils/mod.rs` — add:
   ```rust
   /// Derive a model's config_key from its repo_id.
   ///
   /// WASM-safe mirror of `tama_core::models::ConfigKey::from_repo_id` — the
   /// `tama` crate only links tama-core under the `ssr` feature, so Leptos
   /// components compiled to WASM cannot use the newtype. The rule MUST stay
   /// identical: `repo_id.to_lowercase().replace('/', "--")`.
   pub fn config_key_from_repo_id(repo_id: &str) -> String {
       repo_id.to_lowercase().replace('/', "--")
   }
   ```
   plus a unit test asserting the same vectors as `ConfigKey::from_repo_id` (keeps the mirror honest).
5. `pull_quant_wizard.rs:451` — `repo.replace('/', "--").to_lowercase()` → `crate::utils::config_key_from_repo_id(&repo)` (behavior identical — order swap is a no-op — but now there is one named site).

**Steps:**
- [ ] Write the failing mirror test in `crates/tama/src/utils/mod.rs`
- [ ] Run `cargo nextest run --package tama -- utils` — verify failure (fn doesn't exist)
- [ ] Implement the helper + the four call-site replacements
- [ ] Run `rg 'to_lowercase\(\)\.replace|replace\([^)]*\)\.to_lowercase' crates/tama/src` — zero hits outside `utils/mod.rs` and `crud/delete.rs:254`'s card path (Task 4)
- [ ] Run `cargo nextest run --package tama` — all pass
- [ ] Run `cargo check --package tama --no-default-features --features csr` — the wizard change compiles for WASM-target cfg (type-check only)
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "refactor: route tama-crate config_key derivations through ConfigKey/mirror helper"

**Acceptance criteria:**
- [ ] No inline derivation remains in `crates/tama/src` outside `utils/mod.rs`
- [ ] csr build type-checks (`--no-default-features --features csr`)
- [ ] `cargo nextest run --package tama` passes; clippy clean

---

### Task 4: Give the case-preserving card-filename slug a named function

**Context:**
Model card filenames (`<configs_dir>/<slug>.toml`) use a case-PRESERVING slug — `proxy/state.rs:202-207` builds `format!("{}--{}.toml", org, name)` from the original `model_name`, and `pull/verify.rs:213-214` writes cards with `repo_id.replace('/', "--")` (no lowercase). Cards already exist on disk with mixed-case names, so lowercasing this rule would orphan them — the rule stays, but it must become a named single site so nobody "unifies" it with `ConfigKey` by mistake. Verified sites: `crates/tama-core/src/proxy/state.rs:202-207`, `crates/tama-core/src/proxy/tama_handlers/pull/verify.rs:213`, `crates/tama/src/api/models/crud/delete.rs:254`, and tests `crates/tama-core/src/proxy/tama_handlers/tests.rs:16,42`. Decision: `card_slug` lives in `crates/tama-core/src/models/card.rs` (home of `ModelCard`), re-exported from `models/mod.rs`.

**Files:**
- Modify: `crates/tama-core/src/models/card.rs`
- Modify: `crates/tama-core/src/models/mod.rs`
- Modify: `crates/tama-core/src/proxy/state.rs`
- Modify: `crates/tama-core/src/proxy/tama_handlers/pull/verify.rs`
- Modify: `crates/tama/src/api/models/crud/delete.rs`
- Modify: `crates/tama-core/src/proxy/tama_handlers/tests.rs`

**What to implement:**

1. **`crates/tama-core/src/models/card.rs`** — add:
   ```rust
   /// Filename slug for a model card (`<slug>.toml` in the configs directory).
   ///
   /// Deliberately CASE-PRESERVING (unlike `ConfigKey::from_repo_id`): card
   /// files already exist on disk with mixed-case names, and lowercasing the
   /// rule would orphan them. Never "unify" this with the config_key rule.
   pub fn card_slug(repo_id: &str) -> String {
       repo_id.replace('/', "--")
   }
   ```
   plus `#[cfg(test)]` tests: `"Owner/Repo-GGUF"` → `"Owner--Repo-GGUF"` (case preserved); no-slash input unchanged.
2. **`models/mod.rs`** — extend the existing `pub use card::{ModelCard, ModelMeta, QuantInfo};` to include `card_slug`.
3. **`proxy/state.rs:202-207`** — replace the `split_once('/')` + two-branch `format!` block with `let card_filename = format!("{}.toml", crate::models::card_slug(model_name));` (behavior identical for both slash and no-slash names — `card_slug` handles both; the `unwrap_or(("", model_name))` org-split logic disappears, which is safe because `card_slug("")`… verify: for a name WITHOUT a slash the current code produces `format!("{}.toml", name)` — identical to `card_slug(name)`. For a name WITH a slash: `format!("{}--{}.toml", org, name)` == `card_slug` output. Document the equivalence in the commit message.)
4. **`pull/verify.rs:213`** — `let repo_slug = repo_id.replace('/', "--");` → `let repo_slug = crate::models::card_slug(repo_id);`.
5. **`crates/tama/src/api/models/crud/delete.rs:254`** — `format!("{}.toml", repo_id.replace('/', "--"))` → `format!("{}.toml", tama_core::models::card_slug(&repo_id))`.
6. **`tama_handlers/tests.rs:16,42`** — `repo_id.replace('/', "--")` → `crate::models::card_slug(repo_id)`.

**Steps:**
- [ ] Write the failing `card_slug` tests in `crates/tama-core/src/models/card.rs`
- [ ] Run `cargo nextest run --package tama-core -- models::card` — verify failure
- [ ] Implement `card_slug` + re-export + the five call-site replacements
- [ ] Run `cargo nextest run --package tama-core -- proxy` and `cargo nextest run --package tama-core -- models` — pass (state.rs card-path behavior covered by existing proxy tests; tama_handlers tests use the slug)
- [ ] Run `cargo nextest run --package tama-core` and `cargo nextest run --package tama` — all pass
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "refactor: name the case-preserving card filename slug (card_slug)"

**Acceptance criteria:**
- [ ] `tama_core::models::card_slug` is the only place the card-slug rule is written; all five sites call it
- [ ] Card filenames remain case-preserving (test proves `"Owner/Repo-GGUF"` → `"Owner--Repo-GGUF"`)
- [ ] `cargo nextest run --workspace` passes; clippy clean

---

### Task 5: Rename the two `resolve_model_id` functions to reflect their contracts

**Context:**
Two functions share the name `resolve_model_id` with different semantics: `crates/tama-core/src/proxy/tama_handlers/models/utils.rs:49` (`pub(super) async fn resolve_model_id(state: &ProxyState, raw: &str) -> String` — resolves a raw identifier TO a config_key, scanning the in-memory registry by db_id and normalizing slash-form repo ids) and `crates/tama/src/api/models/info.rs:29` (`pub(crate) fn resolve_model_id(id_str: &str, repo: &Repository) -> anyhow::Result<Option<i64>>` — resolves an identifier TO a DB i64, trying integer parse then config_key→repo_id lookup). Decision: rename the tama-core one to `resolve_config_key` (returns a config key string) and the tama one to `resolve_db_id` (returns a DB id). Both are crate-private (`pub(super)` / `pub(crate)`), so the blast radius is the verified caller list below — no public API break. Update both doc comments to state the contract explicitly.

**Files:**
- Modify: `crates/tama-core/src/proxy/tama_handlers/models/utils.rs`
- Modify: `crates/tama-core/src/proxy/tama_handlers/models/handlers.rs`
- Modify: `crates/tama/src/api/models/info.rs`
- Modify: `crates/tama/src/api/models/files.rs`
- Modify: `crates/tama/src/api/models/crud/rename.rs`
- Modify: `crates/tama/src/api/models/crud/update.rs`
- Modify: `crates/tama/src/api/models/crud/delete.rs`

**What to implement:**

1. **`tama_handlers/models/utils.rs:49`** — rename to `pub(super) async fn resolve_config_key(state: &ProxyState, raw: &str) -> String`; extend its doc comment (lines 38–48, which already documents the 3-step resolution) with a first line: `/// Resolve a raw model identifier (db id, repo id, or config key) to a config_key string.` Update the four call sites in `tama_handlers/models/handlers.rs` (:124, :181, :224, :362) and the `use super::utils::resolve_model_id;` at :11 → `use super::utils::resolve_config_key;`.
2. **`crates/tama/src/api/models/info.rs:29`** — rename to `pub(crate) fn resolve_db_id(id_str: &str, repo: &Repository) -> anyhow::Result<Option<i64>>`; doc comment becomes `/// Resolve a model identifier string (integer DB id or config_key) to the integer DB id.` Note: `info.rs` is re-exported via `pub use info::*;` in `crates/tama/src/api/models/mod.rs:7`, so callers import it as `crate::api::models::resolve_db_id`. Update callers:
   - `info.rs:223` (internal call)
   - `files.rs:11` (`use super::resolve_model_id;` → `use super::resolve_db_id;`), `:58`, `:210`
   - `crud/rename.rs:14` (`use crate::api::models::resolve_model_id;` → `...::resolve_db_id;`), `:43`
   - `crud/update.rs:17`, `:48`, `:131`
   - `crud/delete.rs:13`, `:160`
3. After edits: `rg "resolve_model_id" crates/` must return zero hits.

**Steps:**
- [ ] Run `cargo nextest run --package tama-core -- proxy::tama_handlers` and `cargo nextest run --package tama -- api::models` — green baseline
- [ ] Rename `resolve_model_id` → `resolve_config_key` in tama-core + its 5 touch points
- [ ] Rename `resolve_model_id` → `resolve_db_id` in crates/tama + its 10 touch points
- [ ] Run `rg "resolve_model_id" crates/` — zero hits
- [ ] Run `cargo nextest run --package tama-core` and `cargo nextest run --package tama` — all pass
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "refactor: rename resolve_model_id to resolve_config_key/resolve_db_id"

**Acceptance criteria:**
- [ ] `resolve_config_key` (tama-core, returns config key `String`) and `resolve_db_id` (tama, returns `Result<Option<i64>>`) have distinct names matching their contracts; zero `resolve_model_id` references remain
- [ ] Doc comments on both state the contract
- [ ] `cargo nextest run --workspace` passes; clippy clean
