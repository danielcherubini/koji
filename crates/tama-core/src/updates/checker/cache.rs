use std::sync::Arc;

/// Cache entry: (commit_sha, files, epoch_timestamp)
pub type CacheEntry = (String, Vec<crate::models::pull::RemoteGguf>, i64);

/// In-memory LRU cache for HuggingFace GGUF file listings.
/// Reduces API calls by caching (commit_sha, files) per repo_id for 5 minutes.
pub struct GgufListingCache {
    cache: Arc<tokio::sync::Mutex<lru::LruCache<String, CacheEntry>>>,
}

impl Clone for GgufListingCache {
    fn clone(&self) -> Self {
        Self {
            cache: Arc::clone(&self.cache),
        }
    }
}

impl GgufListingCache {
    const TTL_SECS: i64 = 300; // 5 minutes
    const CAPACITY: usize = 64;

    pub fn new() -> Self {
        Self {
            cache: Arc::new(tokio::sync::Mutex::new(lru::LruCache::new(
                std::num::NonZeroUsize::new(Self::CAPACITY).unwrap(),
            ))),
        }
    }

    /// Get a cached entry if it exists and is fresh (within TTL).
    pub async fn get(
        &self,
        repo_id: &str,
    ) -> Option<(String, Vec<crate::models::pull::RemoteGguf>)> {
        let now = chrono::Utc::now().timestamp();
        let mut cache = self.cache.lock().await;
        if let Some(entry) = cache.get(repo_id) {
            let (sha, files, epoch) = entry;
            if now - *epoch < Self::TTL_SECS {
                return Some((sha.clone(), files.clone()));
            }
            // Stale — remove it so the next call fetches fresh data
            cache.pop(repo_id);
        }
        None
    }

    /// Store a result in the cache with the current timestamp.
    pub async fn insert(
        &self,
        repo_id: String,
        commit_sha: String,
        files: Vec<crate::models::pull::RemoteGguf>,
    ) {
        let now = chrono::Utc::now().timestamp();
        let mut cache = self.cache.lock().await;
        cache.put(repo_id, (commit_sha, files, now));
    }
}

impl Default for GgufListingCache {
    fn default() -> Self {
        Self::new()
    }
}
