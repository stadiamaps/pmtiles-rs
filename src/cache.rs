use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, RwLock};

use crate::{DirEntry, Directory, PmtResult, TileId};

/// Result of a directory cache lookup.
pub enum DirCacheResult {
    /// The directory was not found in the cache.
    NotCached,
    /// The tile was not found in the directory.
    NotFound,
    /// The tile was found in the directory.
    Found(DirEntry),
}

impl From<Option<&DirEntry>> for DirCacheResult {
    fn from(entry: Option<&DirEntry>) -> Self {
        match entry {
            Some(entry) => DirCacheResult::Found(entry.clone()),
            None => DirCacheResult::NotFound,
        }
    }
}

/// A cache for `PMTiles` directories.
pub trait DirectoryCache {
    /// Get a directory entry from the cache, or insert it using the provided fetcher function.
    fn get_dir_entry_or_insert(
        &self,
        offset: usize,
        tile_id: TileId,
        fetcher: impl Future<Output = PmtResult<Directory>> + Send,
    ) -> impl Future<Output = PmtResult<Option<DirEntry>>> + Send;
}

/// A cache that does not cache anything.
pub struct NoCache;

impl DirectoryCache for NoCache {
    #[inline]
    async fn get_dir_entry_or_insert(
        &self,
        _: usize,
        tile_id: TileId,
        fetcher: impl Future<Output = PmtResult<Directory>> + Send,
    ) -> PmtResult<Option<DirEntry>> {
        let dir = fetcher.await?;
        Ok(dir.find_tile_id(tile_id).cloned())
    }
}

/// A simple HashMap-based implementation of a `PMTiles` directory cache.
#[derive(Default)]
pub struct HashMapCache {
    /// The internal cache storage.
    pub cache: Arc<RwLock<HashMap<usize, Directory>>>,
}

impl HashMapCache {
    fn get_dir_entry(&self, offset: usize, tile_id: TileId) -> DirCacheResult {
        // Panic if the lock is poisoned is not something the user can handle
        #[expect(clippy::unwrap_used)]
        if let Some(dir) = self.cache.read().unwrap().get(&offset) {
            return dir.find_tile_id(tile_id).into();
        }
        DirCacheResult::NotCached
    }

    fn insert_dir(&self, offset: usize, directory: Directory) {
        // Panic if the lock is poisoned is not something the user can handle
        #[expect(clippy::unwrap_used)]
        self.cache.write().unwrap().insert(offset, directory);
    }
}

impl DirectoryCache for HashMapCache {
    async fn get_dir_entry_or_insert(
        &self,
        offset: usize,
        tile_id: TileId,
        fetcher: impl Future<Output = PmtResult<Directory>> + Send,
    ) -> PmtResult<Option<DirEntry>> {
        let dir_entry = self.get_dir_entry(offset, tile_id);
        match dir_entry {
            DirCacheResult::Found(entry) => Ok(Some(entry)),
            DirCacheResult::NotFound => Ok(None),
            DirCacheResult::NotCached => {
                let directory = fetcher.await?;
                let dir_entry = directory.find_tile_id(tile_id).cloned();
                self.insert_dir(offset, directory);
                Ok(dir_entry)
            }
        }
    }
}

/// Provides an implementation of `DirectoryCache` using the `moka` crate.
#[cfg(feature = "moka")]
pub struct MokaCache {
    /// This is the internal moka future cache.
    ///
    /// Directories are reference counted so cache hits do not clone their entry storage.
    pub cache: moka::future::Cache<usize, Arc<Directory>>,
}

#[cfg(feature = "moka")]
impl DirectoryCache for MokaCache {
    async fn get_dir_entry_or_insert(
        &self,
        offset: usize,
        tile_id: TileId,
        fetcher: impl Future<Output = PmtResult<Directory>> + Send,
    ) -> PmtResult<Option<DirEntry>> {
        let directory = self
            .cache
            .try_get_with(offset, async { fetcher.await.map(Arc::new) })
            .await;
        let directory = directory.map_err(|e| {
            crate::PmtError::DirectoryCacheError(format!("Moka cache fetch error: {e}"))
        })?;
        Ok(directory.find_tile_id(tile_id).cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "moka")]
    use crate::MokaCache;
    use crate::{DirEntry, Directory, DirectoryCache, HashMapCache, PmtError};
    #[cfg(feature = "moka")]
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[cfg(feature = "moka")]
    use tokio::sync::Notify;

    #[tokio::test]
    async fn test_hash_map_cache() {
        let cache = HashMapCache::default();
        let offset = 0;
        let tile_id = TileId::new(0).unwrap();
        let mut dir_to_cache = Directory::default();
        dir_to_cache.entries.push(DirEntry::default());

        // Initially, the cache should be empty.
        let get_result = cache.get_dir_entry(offset, tile_id);
        assert!(matches!(get_result, DirCacheResult::NotCached));

        // Insert a directory into the cache.
        cache.insert_dir(offset, dir_to_cache);

        // Now, the cache should return Found since the directory contains an entry.
        let get_result = cache.get_dir_entry(offset, tile_id);
        assert!(matches!(get_result, DirCacheResult::Found(_)));

        // The fetcher won't get called, because the entry is already cached.
        let get_result = cache
            .get_dir_entry_or_insert(offset, tile_id, async { Err(PmtError::InvalidEntry) })
            .await
            .unwrap();
        assert!(get_result.is_some());

        // Now the fetcher will be executed.
        let get_result = cache
            .get_dir_entry_or_insert(offset + 10, tile_id, async { Err(PmtError::InvalidEntry) })
            .await;
        assert!(get_result.is_err());

        // The fetcher will be executed and will contain a tile
        let get_result = cache
            .get_dir_entry_or_insert(offset + 10, tile_id, async {
                let mut dir = Directory::default();
                let dir_entry = DirEntry {
                    offset: (offset + 10) as u64,
                    ..Default::default()
                };
                dir.entries.push(dir_entry);
                Ok(dir)
            })
            .await;
        assert!(get_result.is_ok());
        assert!(get_result.unwrap().is_some());
    }

    #[cfg(feature = "moka")]
    #[tokio::test]
    async fn moka_cache_reuses_directory_allocation_on_hits() {
        let cache = MokaCache {
            cache: moka::future::Cache::new(100),
        };
        let offset = 0;
        let tile_id = TileId::new(0).unwrap();
        let mut dir_to_cache = Directory::default();
        dir_to_cache.entries.push(DirEntry::default());

        let get_result = cache
            .get_dir_entry_or_insert(offset, tile_id, async { Ok(dir_to_cache) })
            .await
            .unwrap();
        assert!(get_result.is_some());

        let initially_cached = cache.cache.get(&offset).await.unwrap();

        let get_result = cache
            .get_dir_entry_or_insert(offset, tile_id, async { Err(PmtError::InvalidEntry) })
            .await
            .unwrap();
        assert!(get_result.is_some());

        let cached_after_hit = cache.cache.get(&offset).await.unwrap();
        assert!(Arc::ptr_eq(&initially_cached, &cached_after_hit));
    }

    #[cfg(feature = "moka")]
    #[tokio::test]
    async fn moka_cache_coalesces_concurrent_misses() {
        let cache = Arc::new(MokaCache {
            cache: moka::future::Cache::new(100),
        });
        let fetch_count = Arc::new(AtomicUsize::new(0));
        let fetch_started = Arc::new(Notify::new());
        let release_fetch = Arc::new(Notify::new());
        let tile_id = TileId::new(0).unwrap();

        let first = {
            let cache = Arc::clone(&cache);
            let fetch_count = Arc::clone(&fetch_count);
            let fetch_started = Arc::clone(&fetch_started);
            let release_fetch = Arc::clone(&release_fetch);
            tokio::spawn(async move {
                cache
                    .get_dir_entry_or_insert(0, tile_id, async move {
                        fetch_count.fetch_add(1, Ordering::SeqCst);
                        fetch_started.notify_one();
                        release_fetch.notified().await;
                        let mut directory = Directory::default();
                        directory.entries.push(DirEntry::default());
                        Ok(directory)
                    })
                    .await
            })
        };

        fetch_started.notified().await;

        let second_started = Arc::new(Notify::new());
        let second = {
            let cache = Arc::clone(&cache);
            let fetch_count = Arc::clone(&fetch_count);
            let second_started = Arc::clone(&second_started);
            tokio::spawn(async move {
                second_started.notify_one();
                cache
                    .get_dir_entry_or_insert(0, tile_id, async move {
                        fetch_count.fetch_add(1, Ordering::SeqCst);
                        Ok(Directory::default())
                    })
                    .await
            })
        };

        second_started.notified().await;
        release_fetch.notify_one();

        assert!(first.await.unwrap().unwrap().is_some());
        assert!(second.await.unwrap().unwrap().is_some());
        assert_eq!(fetch_count.load(Ordering::SeqCst), 1);
    }

    #[cfg(feature = "moka")]
    #[tokio::test]
    async fn moka_cache_caches_missing_entries() {
        let cache = MokaCache {
            cache: moka::future::Cache::new(100),
        };
        let tile_id = TileId::new(1).unwrap();

        let first = cache
            .get_dir_entry_or_insert(0, tile_id, async { Ok(Directory::default()) })
            .await
            .unwrap();
        assert!(first.is_none());

        let second = cache
            .get_dir_entry_or_insert(0, tile_id, async { Err(PmtError::InvalidEntry) })
            .await
            .unwrap();
        assert!(second.is_none());
    }

    #[cfg(feature = "moka")]
    #[tokio::test]
    async fn moka_cache_does_not_cache_fetch_errors() {
        let cache = MokaCache {
            cache: moka::future::Cache::new(100),
        };
        let tile_id = TileId::new(0).unwrap();

        let first = cache
            .get_dir_entry_or_insert(0, tile_id, async { Err(PmtError::InvalidEntry) })
            .await;
        assert!(first.is_err());

        let second = cache
            .get_dir_entry_or_insert(0, tile_id, async {
                let mut directory = Directory::default();
                directory.entries.push(DirEntry::default());
                Ok(directory)
            })
            .await
            .unwrap();
        assert!(second.is_some());
    }
}
