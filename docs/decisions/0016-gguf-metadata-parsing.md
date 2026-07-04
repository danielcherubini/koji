# GGUF metadata parsing for authoritative model info

Model metadata (parameter count, context length, architecture, quantization type, etc.) is extracted by parsing the GGUF file header directly, rather than relying on HuggingFace API responses or user input. The proxy reads the GGUF magic number, version, header size, and key-value pairs (e.g. `general.name`, `general.architecture`, `llama.context_length`) to populate the model card with authoritative data.

This replaced the earlier approach where metadata came from the HuggingFace API or was manually entered during the pull wizard. The GGUF header is the source of truth — it is written by the model author at quantization time and travels with the file. API responses can be stale, incomplete, or missing for community uploads.

The parsing happens during the download queue's post-download verification step. After a file is downloaded and verified (size, SHA256), the GGUF header is read and metadata is inserted into the DB. If parsing fails (corrupt file, non-GGUF), the download is marked as failed with the error.

**Status:** accepted

**Considered Options:**

- **HuggingFace API metadata** (status quo) — convenient but stale, incomplete, or missing for community uploads
- **GGUF header parsing** (chosen) — authoritative metadata that travels with the file; parsed post-download
- **User-entered metadata** — flexible but error-prone and inconsistent
- **Infer from filename** (e.g. `model-Q4_K_M.gguf`) — fragile; naming conventions vary across repos

**Consequences:**

- Good, because metadata is always accurate — comes from the file itself, not a third-party API
- Good, because works for any GGUF file regardless of hosting source (HF, local, custom)
- Good, because enables features like VRAM-aware context size suggestions and quant dropdowns
- Bad, because requires downloading the file before metadata is available (can't preview before pull)
- Bad, because GGUF parsing adds a post-download step (blocks queue until complete)
