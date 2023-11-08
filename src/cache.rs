use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::directory::{Directory, Entry};

pub enum SearchResult {
    NotCached,
    NotFound,
    Found(Entry),
}

impl From<Option<&Entry>> for SearchResult {
    fn from(entry: Option<&Entry>) -> Self {
        match entry {
            Some(entry) => SearchResult::Found(entry.clone()),
            None => SearchResult::NotFound,
        }
    }
}

/// A cache for PMTiles directories.
pub trait DirectoryCache {
    /// Get a directory from the cache, using the offset as a key.
    fn get_dir_entry(&self, offset: usize, tile_id: u64) -> SearchResult;

    /// Insert a directory into the cache, using the offset as a key.
    /// Note that cache must be internally mutable.
    fn insert_dir(&self, offset: usize, directory: Directory);
}

pub struct NoCache;

impl DirectoryCache for NoCache {
    #[inline]
    fn get_dir_entry(&self, _offset: usize, _tile_id: u64) -> SearchResult {
        SearchResult::NotCached
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
    fn get_dir_entry(&self, offset: usize, tile_id: u64) -> SearchResult {
        if let Some(dir) = self.cache.lock().unwrap().get(&offset) {
            return dir.find_tile_id(tile_id).into();
        }
        SearchResult::NotCached
    }

    fn insert_dir(&self, offset: usize, directory: Directory) {
        self.cache.lock().unwrap().insert(offset, directory);
    }
}
