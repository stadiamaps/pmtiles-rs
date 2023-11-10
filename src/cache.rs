use std::collections::HashMap;
use std::sync::{Arc, Mutex};

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

/// A cache for PMTiles directories.
pub trait DirectoryCache {
    /// Get a directory from the cache, using the offset as a key.
    fn get_dir_entry(&self, offset: usize, tile_id: u64) -> DirCacheResult;

    /// Insert a directory into the cache, using the offset as a key.
    /// Note that cache must be internally mutable.
    fn insert_dir(&self, offset: usize, directory: Directory);
}

pub struct NoCache;

impl DirectoryCache for NoCache {
    #[inline]
    fn get_dir_entry(&self, _offset: usize, _tile_id: u64) -> DirCacheResult {
        DirCacheResult::NotCached
    }

    #[inline]
    fn insert_dir(&self, _offset: usize, _directory: Directory) {}
}

/// A simple HashMap-based implementation of a PMTiles directory cache.
#[derive(Default)]
pub struct HashMapCache {
    pub cache: Arc<Mutex<HashMap<usize, Directory>>>,
}

impl DirectoryCache for HashMapCache {
    fn get_dir_entry(&self, offset: usize, tile_id: u64) -> DirCacheResult {
        if let Some(dir) = self.cache.lock().unwrap().get(&offset) {
            return dir.find_tile_id(tile_id).into();
        }
        DirCacheResult::NotCached
    }

    fn insert_dir(&self, offset: usize, directory: Directory) {
        self.cache.lock().unwrap().insert(offset, directory);
    }
}
