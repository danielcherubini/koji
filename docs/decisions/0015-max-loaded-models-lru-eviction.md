# Max loaded models with LRU eviction

The proxy caps concurrent model loads per GPU via `max_loaded_models` (default: 1). When the cap is reached and a new model needs to load, the least-recently-used model on that GPU is evicted (unloaded). The "last accessed" timestamp updates on every incoming request, so actively used models stay loaded.

This is a per-GPU cap, not global — with `max_loaded_models = 1` and 2 GPUs, up to 2 models can be loaded (one per GPU). The cap respects GPU assignment: only models assigned to the same GPU compete for slots. Models without GPU assignment (running on CPU or auto-assigned) share a separate pool.

The LRU strategy was chosen over FIFO or random eviction because it matches the access pattern of AI clients — recently used models are likely to be requested again soon, while idle models waste VRAM. The `auto_unload` boolean (replacing the old `idle_timeout_secs=0` convention) controls whether models unload on idle timeout or stay loaded until evicted.

**Status:** accepted

**Considered Options:**

- **No cap** (load everything) — simplest but risks OOM on VRAM-constrained systems
- **LRU eviction per GPU** (chosen) — matches access patterns; recently used models stay loaded, idle models free VRAM
- **FIFO eviction** — ignores access patterns; a model loaded first but used constantly would be evicted before an idle model
- **Manual unload only** — puts burden on the user; Tama's design goal is automatic management
- **OS-level OOM killer** — unpredictable; kills random processes rather than gracefully unloading models

**Consequences:**

- Good, because prevents VRAM exhaustion — cap ensures predictable memory usage
- Good, because LRU matches real access patterns — active models stay loaded
- Good, because per-GPU caps respect physical resource boundaries
- Bad, because eviction causes reload latency when a model is requested again
- Bad, because `last_accessed` tracking adds overhead on every proxied request
