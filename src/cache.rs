use std::collections::HashMap;
use std::future::Future;
use std::sync::RwLock;
#[cfg(feature = "moka")]
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

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
///
/// Directories are identified by their `offset` in the archive, so each cache instance must
/// be used by a single archive. A cache whose storage is shared by several archives must also
/// key directories by archive, as [`SharedMokaCache`](crate::SharedMokaCache) does.
pub trait DirectoryCache {
    /// Get a directory entry from the cache, or insert it using the provided fetcher function.
    fn get_dir_entry_or_insert(
        &self,
        offset: u64,
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
        _: u64,
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
    cache: RwLock<HashMap<u64, Directory>>,
}

impl HashMapCache {
    fn get_dir_entry(&self, offset: u64, tile_id: TileId) -> DirCacheResult {
        // Panic if the lock is poisoned is not something the user can handle
        #[expect(clippy::unwrap_used)]
        if let Some(dir) = self.cache.read().unwrap().get(&offset) {
            return dir.find_tile_id(tile_id).into();
        }
        DirCacheResult::NotCached
    }

    fn insert_dir(&self, offset: u64, directory: Directory) {
        // Panic if the lock is poisoned is not something the user can handle
        #[expect(clippy::unwrap_used)]
        self.cache.write().unwrap().insert(offset, directory);
    }
}

impl DirectoryCache for HashMapCache {
    async fn get_dir_entry_or_insert(
        &self,
        offset: u64,
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

/// A [`DirectoryCache`] for one archive, using the `moka` crate.
///
/// Directories are stored in [`Arc`]s, so a cache hit does not copy the directory's entries.
/// Concurrent misses for the same directory fetch it only once, and failed fetches are not cached.
///
/// To give several archives one memory budget, create their caches with
/// [`SharedMokaCache::new_cache`]. Caches never share directories, even for the same archive,
/// so to read one archive from several places, share its reader instead (e.g. in an [`Arc`]).
/// For other moka settings, such as expiration or eviction listeners, implement
/// [`DirectoryCache`] for your own type.
///
/// ```
/// use pmtiles::MokaCache;
///
/// // Holds up to 100 directories.
/// let cache = MokaCache::new(100);
///
/// // Holds up to 64 MiB of directories.
/// let cache = MokaCache::with_max_bytes(64 * 1024 * 1024);
/// ```
#[cfg(feature = "moka")]
pub struct MokaCache {
    cache: moka::future::Cache<(u64, u64), Arc<Directory>>,
    archive_id: u64,
}

#[cfg(feature = "moka")]
impl MokaCache {
    /// Creates a cache that holds up to `max_capacity` directories.
    #[must_use]
    pub fn new(max_capacity: u64) -> Self {
        SharedMokaCache::new(max_capacity).new_cache()
    }

    /// Creates a cache that holds up to about `max_bytes` bytes of directories.
    ///
    /// A directory larger than `max_bytes` is never cached. Leaf directories often take
    /// 100 KB or more each, so the budget should fit several of them.
    #[must_use]
    pub fn with_max_bytes(max_bytes: u64) -> Self {
        SharedMokaCache::with_max_bytes(max_bytes).new_cache()
    }

    /// Returns the approximate number of cached directories, including those of other
    /// archives that share this cache's storage.
    #[must_use]
    pub fn entry_count(&self) -> u64 {
        self.cache.entry_count()
    }

    /// Returns the approximate total weight of cached directories, including those of other
    /// archives that share this cache's storage.
    ///
    /// This is the number of bytes for a cache created with a byte budget, and the number of
    /// directories otherwise.
    #[must_use]
    pub fn weighted_size(&self) -> u64 {
        self.cache.weighted_size()
    }
}

#[cfg(feature = "moka")]
impl DirectoryCache for MokaCache {
    async fn get_dir_entry_or_insert(
        &self,
        offset: u64,
        tile_id: TileId,
        fetcher: impl Future<Output = PmtResult<Directory>> + Send,
    ) -> PmtResult<Option<DirEntry>> {
        let directory = self
            .cache
            .try_get_with_by_ref(&(self.archive_id, offset), async {
                fetcher.await.map(Arc::new)
            })
            .await;
        let directory = directory.map_err(|e| {
            crate::PmtError::DirectoryCacheError(format!("Moka cache fetch error: {e}"))
        })?;
        Ok(directory.find_tile_id(tile_id).cloned())
    }
}

/// Moka storage shared by the [`MokaCache`]s of several archives, so they have one memory budget.
///
/// Each archive needs its own cache from [`SharedMokaCache::new_cache`], which keeps its
/// directories apart from those of other archives.
///
/// ```
/// use pmtiles::SharedMokaCache;
///
/// let shared = SharedMokaCache::with_max_bytes(256 * 1024 * 1024);
/// // Give each cache to the reader of a different archive.
/// let first = shared.new_cache();
/// let second = shared.new_cache();
/// ```
#[cfg(feature = "moka")]
#[derive(Clone)]
pub struct SharedMokaCache {
    cache: moka::future::Cache<(u64, u64), Arc<Directory>>,
}

#[cfg(feature = "moka")]
impl SharedMokaCache {
    /// Creates storage that holds up to `max_capacity` directories.
    #[must_use]
    pub fn new(max_capacity: u64) -> Self {
        Self {
            cache: moka::future::Cache::new(max_capacity),
        }
    }

    /// Creates storage that holds up to about `max_bytes` bytes of directories.
    ///
    /// A directory larger than `max_bytes` is never cached. Leaf directories often take
    /// 100 KB or more each, so the budget should fit several of them.
    #[must_use]
    pub fn with_max_bytes(max_bytes: u64) -> Self {
        Self {
            cache: moka::future::Cache::builder()
                .weigher(|_key, directory: &Arc<Directory>| {
                    u32::try_from(directory.get_approx_byte_size()).unwrap_or(u32::MAX)
                })
                .max_capacity(max_bytes)
                .build(),
        }
    }

    /// Creates a cache for one archive that uses this storage.
    #[must_use]
    pub fn new_cache(&self) -> MokaCache {
        static NEXT_ARCHIVE_ID: AtomicU64 = AtomicU64::new(0);
        MokaCache {
            cache: self.cache.clone(),
            archive_id: NEXT_ARCHIVE_ID.fetch_add(1, Ordering::Relaxed),
        }
    }

    /// Returns the approximate number of cached directories of all archives.
    #[must_use]
    pub fn entry_count(&self) -> u64 {
        self.cache.entry_count()
    }

    /// Returns the approximate total weight of cached directories of all archives.
    ///
    /// This is the number of bytes for storage created with a byte budget, and the number of
    /// directories otherwise.
    #[must_use]
    pub fn weighted_size(&self) -> u64 {
        self.cache.weighted_size()
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "moka")]
    use std::sync::atomic::AtomicUsize;

    #[cfg(feature = "moka")]
    use tokio::sync::Notify;

    use super::*;
    use crate::{DirEntry, Directory, DirectoryCache, HashMapCache, PmtError};
    #[cfg(feature = "moka")]
    use crate::{MokaCache, SharedMokaCache};

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
                    offset: offset + 10,
                    ..Default::default()
                };
                dir.entries.push(dir_entry);
                Ok(dir)
            })
            .await;
        assert!(get_result.is_ok());
        assert!(get_result.unwrap().is_some());
    }

    /// Offsets are keyed by their full `u64` value, so two offsets that differ only
    /// above the 32-bit boundary map to distinct cache slots.
    #[tokio::test]
    async fn hash_map_cache_keys_on_full_u64_offset() {
        let cache = HashMapCache::default();
        let tile_id = TileId::new(0).unwrap();

        let low = 0u64;
        let high = 1u64 << 32; // 4 GiB; shares the low 32 bits with `low`

        // Cache a directory at the low offset only.
        let mut dir = Directory::default();
        dir.entries.push(DirEntry::default());
        cache.insert_dir(low, dir);

        // The low offset is a hit...
        assert!(matches!(
            cache.get_dir_entry(low, tile_id),
            DirCacheResult::Found(_)
        ));
        // ...while the high offset is a distinct, uncached slot.
        assert!(matches!(
            cache.get_dir_entry(high, tile_id),
            DirCacheResult::NotCached
        ));
    }

    #[cfg(feature = "moka")]
    /// A hit returns the cached directory without calling the fetcher or replacing the entry.
    #[tokio::test]
    async fn moka_cache_hit_keeps_cached_directory() {
        let cache = MokaCache::new(100);
        let offset = 0;
        let tile_id = TileId::new(0).unwrap();
        let mut dir_to_cache = Directory::default();
        dir_to_cache.entries.push(DirEntry::default());

        let get_result = cache
            .get_dir_entry_or_insert(offset, tile_id, async { Ok(dir_to_cache) })
            .await
            .unwrap();
        assert!(get_result.is_some());

        let key = (cache.archive_id, offset);
        let initially_cached = cache.cache.get(&key).await.unwrap();

        let get_result = cache
            .get_dir_entry_or_insert(offset, tile_id, async { Err(PmtError::InvalidEntry) })
            .await
            .unwrap();
        assert!(get_result.is_some());

        let cached_after_hit = cache.cache.get(&key).await.unwrap();
        assert!(Arc::ptr_eq(&initially_cached, &cached_after_hit));
    }

    #[cfg(feature = "moka")]
    /// Offsets are keyed by their full `u64` value, so two offsets that differ only
    /// above the 32-bit boundary map to distinct cache slots.
    #[tokio::test]
    async fn moka_cache_keys_on_full_u64_offset() {
        let cache = MokaCache::new(100);
        let tile_id = TileId::new(0).unwrap();

        let low = 0u64;
        let high = 1u64 << 32; // 4 GiB; shares the low 32 bits with `low`

        // Cache a directory containing the tile at the low offset.
        let found = cache
            .get_dir_entry_or_insert(low, tile_id, async {
                let mut dir = Directory::default();
                dir.entries.push(DirEntry::default());
                Ok(dir)
            })
            .await
            .unwrap();
        assert!(found.is_some());

        // The high offset is a distinct key, so its fetcher runs and returns an empty
        // directory. A collision would instead return the cached low entry.
        let found = cache
            .get_dir_entry_or_insert(high, tile_id, async { Ok(Directory::default()) })
            .await
            .unwrap();
        assert!(found.is_none());
    }

    #[cfg(feature = "moka")]
    #[tokio::test(flavor = "current_thread")]
    async fn moka_cache_coalesces_concurrent_misses() {
        let cache = Arc::new(MokaCache::new(100));
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

        let second = {
            let cache = Arc::clone(&cache);
            let fetch_count = Arc::clone(&fetch_count);
            tokio::spawn(async move {
                cache
                    .get_dir_entry_or_insert(0, tile_id, async move {
                        fetch_count.fetch_add(1, Ordering::SeqCst);
                        Ok(Directory::default())
                    })
                    .await
            })
        };

        // On the current-thread runtime, yielding runs the second task until it waits
        // for the first fetch to finish.
        tokio::task::yield_now().await;
        release_fetch.notify_one();

        assert!(first.await.unwrap().unwrap().is_some());
        assert!(second.await.unwrap().unwrap().is_some());
        assert_eq!(fetch_count.load(Ordering::SeqCst), 1);
    }

    #[cfg(feature = "moka")]
    #[tokio::test]
    async fn moka_cache_caches_directory_without_tile() {
        let cache = MokaCache::new(100);
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
        let cache = MokaCache::new(100);
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

    #[cfg(feature = "moka")]
    #[tokio::test]
    async fn shared_moka_cache_keeps_archives_apart() {
        let shared = SharedMokaCache::new(100);
        let first = shared.new_cache();
        let second = shared.new_cache();
        let tile_id = TileId::new(0).unwrap();

        let found = first
            .get_dir_entry_or_insert(0, tile_id, async {
                let mut directory = Directory::default();
                directory.entries.push(DirEntry::default());
                Ok(directory)
            })
            .await
            .unwrap();
        assert!(found.is_some());

        // The same offset in another archive is a different directory.
        let other = second
            .get_dir_entry_or_insert(0, tile_id, async { Ok(Directory::default()) })
            .await
            .unwrap();
        assert!(other.is_none());

        let cached = first
            .get_dir_entry_or_insert(0, tile_id, async { Err(PmtError::InvalidEntry) })
            .await
            .unwrap();
        assert!(cached.is_some());

        // Statistics cover the shared storage.
        shared.cache.run_pending_tasks().await;
        assert_eq!(shared.entry_count(), 2);
        assert_eq!(shared.weighted_size(), 2);
        assert_eq!(first.entry_count(), 2);
        assert_eq!(second.weighted_size(), 2);
    }

    #[cfg(feature = "moka")]
    #[tokio::test]
    async fn moka_cache_with_max_bytes_weighs_directories() {
        let directory = || Directory {
            entries: vec![DirEntry::default(); 100],
        };
        let directory_bytes = directory().get_approx_byte_size() as u64;
        // Room for one directory, but not for two.
        let cache = MokaCache::with_max_bytes(directory_bytes * 3 / 2);
        let tile_id = TileId::new(0).unwrap();

        for offset in [0, 1] {
            cache
                .get_dir_entry_or_insert(offset, tile_id, async { Ok(directory()) })
                .await
                .unwrap();
        }
        cache.cache.run_pending_tasks().await;

        assert_eq!(cache.entry_count(), 1);
        assert_eq!(cache.weighted_size(), directory_bytes);
    }
}
