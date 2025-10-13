use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, RwLock};

use crate::{DirEntry, Directory, TileId};

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
