use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, RwLock};

use crate::cache::CacheSlotState::Filled;
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
    /// Get a directory from the cache, using the offset as a key.
    fn get_dir_entry(
        &self,
        offset: usize,
        tile_id: TileId,
    ) -> impl Future<Output = DirCacheResult> + Send;

    /// Insert a directory into the cache, using the offset as a key.
    /// Note that the cache must be internally mutable.
    fn insert_dir(&self, offset: usize, directory: Directory) -> impl Future<Output = ()> + Send;
}

/// The V2 version of the DirectoryCache trait, allows for request coalescing, preventing
/// superfluous inserts when concurrent requests are made for a directory not yet in the cache.
pub trait DirectoryCacheV2 {
    /// Get a directory entry from the cache, or insert it using the provided fetcher function.
    fn get_dir_entry_or_insert(
        &self,
        offset: usize,
        tile_id: TileId,
        fetcher: impl Future<Output = PmtResult<Directory>>,
    ) -> impl Future<Output = PmtResult<Option<DirEntry>>>;
}

/// Provides a blanket implementation of DirectoryCacheV2 for any existing DirectoryCache
/// implementation.
impl<T> DirectoryCacheV2 for T
where
    T: DirectoryCache + Send + Sync,
{
    async fn get_dir_entry_or_insert(
        &self,
        offset: usize,
        tile_id: TileId,
        fetcher: impl Future<Output = PmtResult<Directory>>,
    ) -> PmtResult<Option<DirEntry>> {
        let dir_result = self.get_dir_entry(offset, tile_id).await;
        match dir_result {
            DirCacheResult::Found(dir) => Ok(Some(dir)),
            DirCacheResult::NotFound => Ok(None),
            DirCacheResult::NotCached => {
                let dir = fetcher.await?;
                self.insert_dir(offset, dir.clone()).await;
                let entry = dir.find_tile_id(tile_id);
                Ok(entry.map(|e| e.clone()))
            }
        }
    }
}

/// A cache that does not cache anything.
pub struct NoCache;

impl DirectoryCache for NoCache {
    #[inline]
    async fn get_dir_entry(&self, _offset: usize, _tile_id: TileId) -> DirCacheResult {
        DirCacheResult::NotCached
    }

    #[inline]
    async fn insert_dir(&self, _offset: usize, _directory: Directory) {}
}

/// A simple HashMap-based implementation of a `PMTiles` directory cache.
#[derive(Default)]
pub struct HashMapCache {
    /// The internal cache storage.
    pub cache: Arc<RwLock<HashMap<usize, Directory>>>,
}

impl DirectoryCache for HashMapCache {
    async fn get_dir_entry(&self, offset: usize, tile_id: TileId) -> DirCacheResult {
        // Panic if the lock is poisoned is not something the user can handle
        #[expect(clippy::unwrap_used)]
        if let Some(dir) = self.cache.read().unwrap().get(&offset) {
            return dir.find_tile_id(tile_id).into();
        }
        DirCacheResult::NotCached
    }

    async fn insert_dir(&self, offset: usize, directory: Directory) {
        // Panic if the lock is poisoned is not something the user can handle
        #[expect(clippy::unwrap_used)]
        self.cache.write().unwrap().insert(offset, directory);
    }
}

/// Provides an implementation of the HashMap-based directory cache for the DirectoryCacheV2 trait.
/// The original version of the HashMapCache is kept intact for testing compatibility with existing
/// implementations of the DirectoryCache trait.
/// This implementation uses two levels of locking to allow requests for a slot to coalesce without
/// having to a lock global to the cache.
#[derive(Default)]
pub struct HashMapCacheV2 {
    /// The internal cache storage.
    cache: Arc<tokio::sync::RwLock<HashMap<usize, Arc<CacheSlot>>>>,
}

#[derive(Default)]
enum CacheSlotState {
    /// The slot has been inserted but not yet initialized.
    #[default]
    Empty,
    /// The slot has been initialized with a directory.
    Filled(Directory),
}

#[derive(Default)]
struct CacheSlot {
    /// A slot in the cache. We use a RwLock to guard the slot state and coalesce requests.
    slot: tokio::sync::RwLock<CacheSlotState>,
}

impl CacheSlot {
    async fn get_dir_entry_or_insert(
        &self,
        tile_id: TileId,
        fetcher: impl Future<Output = PmtResult<Directory>>,
    ) -> PmtResult<Option<DirEntry>> {
        {
            // First, get the read lock and check to see if the slot already has been marked as
            // NotFound or Filled.
            let slot_status = self.slot.read().await;
            match &*slot_status {
                Filled(dir) => {
                    // Already filled, return the entry if found.
                    return Ok(dir.find_tile_id(tile_id).cloned());
                }
                // If empty, we need to fetch and fill it.
                _ => {}
            }
        }
        // Now get the write lock to possibly initialize the slot.
        let mut slot_status = self.slot.write().await;
        if let CacheSlotState::Empty = *slot_status {
            let dir = fetcher.await?;
            let dir_entry = dir.find_tile_id(tile_id).cloned();
            *slot_status = Filled(dir);
            return Ok(dir_entry);
        }
        match *slot_status {
            Filled(ref dir) => Ok(dir.find_tile_id(tile_id).cloned()),
            _ => unreachable!(),
        }
    }
}

impl DirectoryCacheV2 for HashMapCacheV2 {
    async fn get_dir_entry_or_insert(
        &self,
        offset: usize,
        tile_id: TileId,
        fetcher: impl Future<Output = PmtResult<Directory>>,
    ) -> PmtResult<Option<DirEntry>> {
        // If we already have a cache slot, use it.
        if let Some(cache_slot) = self.cache.read().await.get(&offset) {
            return cache_slot.get_dir_entry_or_insert(tile_id, fetcher).await;
        }

        let cache_slot: Arc<CacheSlot>;
        {
            // Now get the write lock to possibly insert a new slot.
            let mut wg = self.cache.write().await;
            // Check again after acquiring the write lock.
            if let Some(slot) = wg.get(&offset) {
                cache_slot = slot.clone();
            } else {
                // Insert a new slot.
                let new_slot = Arc::new(CacheSlot::default());
                wg.insert(offset, new_slot.clone());
                cache_slot = new_slot;
            }
        }

        // Now we have a cache_slot, either because it was already there or we just inserted it, and
        // we no longer hold the global cache write lock, so now we can check and initialize the
        // slot itself while only holding the slot lock.
        cache_slot.get_dir_entry_or_insert(tile_id, fetcher).await
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        DirEntry, Directory, DirectoryCache, DirectoryCacheV2, HashMapCache, HashMapCacheV2,
    };

    #[tokio::test]
    async fn test_hash_map_cache() {
        let cache = HashMapCache::default();
        let offset = 0;
        let tile_id = crate::TileId::new(0);
        let mut dir_to_cache = Directory::default();
        dir_to_cache.entries.push(DirEntry::default());

        // Initially, the cache should be empty.
        let get_result = cache.get_dir_entry(offset, tile_id.unwrap()).await;
        assert!(matches!(
            get_result,
            crate::cache::DirCacheResult::NotCached
        ));

        // Insert a directory into the cache.
        cache.insert_dir(offset, dir_to_cache).await;

        // Now, the cache should return NotFound since the directory is empty.
        let get_result = cache.get_dir_entry(offset, tile_id.unwrap()).await;
        assert!(matches!(get_result, crate::cache::DirCacheResult::Found(_)));

        // The fetcher won't get called, because the entry is already cached.
        let get_result = cache
            .get_dir_entry_or_insert(offset, tile_id.unwrap(), async {
                Err(crate::PmtError::InvalidEntry)
            })
            .await
            .unwrap();
        assert!(get_result.is_some());

        // Now the fetcher will be executed.
        let get_result = cache
            .get_dir_entry_or_insert(offset + 10, tile_id.unwrap(), async {
                Err(crate::PmtError::InvalidEntry)
            })
            .await;
        assert!(get_result.is_err());

        // The fetcher will be executed and will contain a tile
        let get_result = cache
            .get_dir_entry_or_insert(offset + 10, tile_id.unwrap(), async {
                let mut dir = Directory::default();
                let mut dir_entry = DirEntry::default();
                dir_entry.offset = (offset + 10) as u64;
                dir.entries.push(dir_entry);
                Ok(dir)
            })
            .await;
        assert!(get_result.is_ok());
        assert!(get_result.unwrap().is_some());
    }

    #[tokio::test]
    async fn test_hash_map_v2_cache() {
        let cache = HashMapCacheV2::default();
        let offset = 0;
        let tile_id = crate::TileId::new(0);
        let mut dir_to_cache = Directory::default();
        dir_to_cache.entries.push(DirEntry::default());

        // Returns an Err
        let get_result = cache
            .get_dir_entry_or_insert(offset, tile_id.unwrap(), async {
                Err(crate::PmtError::InvalidEntry)
            })
            .await;
        assert!(get_result.is_err());

        // Now inserts the directory into the cache and returns the DirEntry.
        let get_result = cache
            .get_dir_entry_or_insert(offset, tile_id.unwrap(), async {
                let mut dir = Directory::default();
                let mut dir_entry = DirEntry::default();
                dir_entry.offset = (offset + 10) as u64;
                dir.entries.push(dir_entry);
                Ok(dir)
            })
            .await;
        assert!(get_result.is_ok());
        assert!(get_result.unwrap().is_some());

        // Repeating the request with the fetcher that returns an Err, but this time the fetcher
        // will not be called because the Directory is cached.
        let get_result = cache
            .get_dir_entry_or_insert(offset, tile_id.unwrap(), async {
                Err(crate::PmtError::InvalidEntry)
            })
            .await;
        assert!(get_result.is_ok());
        assert!(get_result.unwrap().is_some());
    }
}
