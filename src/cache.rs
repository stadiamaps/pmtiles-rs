use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, RwLock};

use crate::directory::{DirEntry, Directory};

pub enum DirCacheResult {
    NotCached,
    NotFound,
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
        tile_id: u64,
    ) -> impl Future<Output = DirCacheResult> + Send;

    /// Insert a directory into the cache, using the offset as a key.
    /// Note that cache must be internally mutable.
    fn insert_dir(&self, offset: usize, directory: Directory) -> impl Future<Output = ()> + Send;
}

pub struct NoCache;

// TODO: Remove #[allow] after switching to Rust/Clippy v1.78+ in CI
//       See https://github.com/rust-lang/rust-clippy/pull/12323
#[allow(clippy::no_effect_underscore_binding)]
impl DirectoryCache for NoCache {
    #[inline]
    async fn get_dir_entry(&self, _offset: usize, _tile_id: u64) -> DirCacheResult {
        DirCacheResult::NotCached
    }

    #[inline]
    async fn insert_dir(&self, _offset: usize, _directory: Directory) {}
}

/// A simple HashMap-based implementation of a `PMTiles` directory cache.
#[derive(Default)]
pub struct HashMapCache {
    pub cache: Arc<RwLock<HashMap<usize, Directory>>>,
}

impl DirectoryCache for HashMapCache {
    async fn get_dir_entry(&self, offset: usize, tile_id: u64) -> DirCacheResult {
        if let Some(dir) = self.cache.read().unwrap().get(&offset) {
            return dir.find_tile_id(tile_id).into();
        }
        DirCacheResult::NotCached
    }

    async fn insert_dir(&self, offset: usize, directory: Directory) {
        self.cache.write().unwrap().insert(offset, directory);
    }
}
