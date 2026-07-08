# API keys use a `tama_` prefix to distinguish from OAuth2 tokens

The auth middleware receives all bearer tokens via the standard `Authorization: Bearer` header. To distinguish Tama API keys from OAuth2/Authentik tokens (which arrive the same way), we use a prefix-based check: tokens starting with `tama_` are validated against the local DB hash table; all other tokens flow through the existing OAuth2/Authentik validation chain.

**Considered Options:** Separate `X-API-Key` header (rejected — breaks OpenAI client compatibility), try-DB-first for all tokens (rejected — adds unnecessary DB lookup on every request), or prefix-based distinction (chosen — zero overhead for non-matching tokens, single header for all clients).

**Consequences:** Changing the prefix format later would break all existing keys. The `tama_` prefix is project-specific and won't collide with other providers (OpenAI `sk-`, Anthropic `sk-ant-`, etc.).
