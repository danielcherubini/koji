# Safetensors repos are pulled via the `hf` CLI subprocess

GGUF pulls go through Tama's built-in in-process downloader (parallel chunks, resume,
SHA-256 verification against LFS hashes, per-file `model_files` tracking). When we added
safetensors (transformers) repo support to the model wizard, we chose NOT to reuse that
pipeline: the wizard shells out to `hf download <repo> --local-dir <models_dir>/<org>/<repo>`
(huggingface_hub's CLI) as a tracked subprocess instead.

**Why:** a transformers repo has no meaningful per-file selection — the whole repo *is* the
model (weight shards + config.json + tokenizers), so per-file job tracking buys little. The
`hf` CLI already provides parallel fetching, resumability, and HF token auth for the
whole-repo case, and this matches the original scoping in plan-081 ("weights pulled via
HF CLI"). The trade-off we accepted: no SHA-256 verification of downloaded files and no
`model_files` DB rows for these weights (re-verify is not available for CLI-pulled files).

**Considered Options:**
- Built-in downloader fetching all repo files — rejected: would need a file whitelist
  (or junk-tolerant "fetch everything"), shard grouping, and per-file jobs for a step the
  UI can't meaningfully expose; kept for GGUF where per-file selection is real.
- Convert safetensors → GGUF in-Tama — rejected: Python/torch host dependency, long
  conversion times, and it duplicates what vLLM already consumes natively.

**Consequences:**
- Hosts need `hf` installed (`pip install -U huggingface_hub`); the pull endpoint 422s
  with the install hint when it's missing.
- Repo pulls are wizard-scoped in-memory jobs (no DB rows, not in the Downloads Center);
  a Tama restart orphans the `hf` process harmlessly (re-pull resumes).
- Two download code paths now coexist — intentional, per the rationale above.
