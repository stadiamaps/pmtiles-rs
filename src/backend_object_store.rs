//! Object store backend implementation using the [`object_store`] crate.
//!
//! This backend provides a unified interface for accessing `PMTiles` from various storage systems including:
//! - AWS S3,
//! - Azure Blob Storage,
//! - Google Cloud Storage,
//! - local files,
//! - HTTP/WebDAV Storage,
//! - memory and
//! - custom implementations

use std::ops::Range;

use bytes::Bytes;
use object_store::ObjectStore;
use object_store::path::Path;
use url::Url;

use crate::{AsyncBackend, PmtError, PmtResult};

/// Backend implementation using the [`object_store`] crate for unified storage access.
///
/// This backend can work with any storage system supported by [`object_store`]:
/// - [AWS S3](https://aws.amazon.com/s3/)
/// - [Azure Blob Storage](https://azure.microsoft.com/en-us/services/storage/blobs/)
/// - [Google Cloud Storage](https://cloud.google.com/storage)
/// - Local files
/// - [HTTP/WebDAV Storage](https://datatracker.ietf.org/doc/html/rfc2518)
/// - Memory
/// - Custom implementations in your/other crates (like [`object_store_opendal`](https://crates.io/crates/object_store_opendal), [`hdfs_native_object_store`](https://crates.io/crates/hdfs_native_object_store), ...)
///
/// # Example
///
/// ```rust
/// use object_store::memory::InMemory;
/// use pmtiles::ObjectStoreBackend;
///
/// let store = Box::new(InMemory::new());
/// let backend = ObjectStoreBackend::new(&url).unwrap();
/// # assert_eq!(backend.store().to_string(), "InMemory")
/// # assert_eq!(backend.path().as_ref(), "tiles.pmtiles")
/// ```
#[derive(Debug)]
pub struct ObjectStoreBackend {
    store: Box<dyn ObjectStore>,
    path: Path,
}

impl ObjectStoreBackend {
    /// Create a new [`ObjectStoreBackend`].
    ///
    /// # Arguments
    /// * `store` - An object store implementation
    /// * `path` - Path to the file within the store
    ///
    /// # Example
    ///
    /// ```rust
    /// use object_store::memory::InMemory;
    /// use pmtiles::ObjectStoreBackend;
    ///
    /// let store = Box::new(InMemory::new());
    /// let backend = ObjectStoreBackend::new(store, "tiles.pmtiles");
    /// # assert_eq!(backend.store().to_string(), "InMemory")
    /// # assert_eq!(backend.path().as_ref(), "tiles.pmtiles")
    /// ```
    ///
    /// You can also parse urls from urls as following.
    /// The supported url schemes are dependent on the [`object_store`]-features.
    /// See [`object_store::parse_url`] for further details.
    ///
    /// ```rust
    /// use pmtiles::ObjectStoreBackend;
    /// use url::Url;
    ///
    /// let url = Url::parse("memory://tiles.pmtiles").unwrap();
    /// let (url, path) = object_store::parse_url(&url);
    /// let backend = ObjectStoreBackend::new(&url).unwrap();
    /// # assert_eq!(backend.store().to_string(), "InMemory")
    /// # assert_eq!(backend.path().as_ref(), "tiles.pmtiles")
    /// ```
    pub fn new<P: Into<Path>>(store: Box<dyn ObjectStore>, path: P) -> Self {
        Self {
            store,
            path: path.into(),
        }
    }

    /// Reference to the underlying object store.
    #[must_use]
    pub fn store(&self) -> &dyn ObjectStore {
        &self.store
    }

    /// The path to the file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl AsyncBackend for ObjectStoreBackend {
    async fn read(&self, offset: usize, length: usize) -> PmtResult<Bytes> {
        let range = Range {
            start: offset as u64,
            end: offset as u64 + length as u64,
        };

        let result = self.store.get_range(&self.path, range).await?;

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use object_store::memory::InMemory;

    use super::*;

    #[test]
    fn test_new_backend() {
        let store = Box::new(InMemory::new());
        let backend = ObjectStoreBackend::new(store, "test.pmtiles");

        assert_eq!(backend.path().as_ref(), "test.pmtiles");
        assert_eq!(backend.store().to_string(), "InMemory");
    }

    #[tokio::test]
    async fn test_error_nonexistant() {
        let store = Box::new(InMemory::new());
        let backend = ObjectStoreBackend::new(store, "nonexistent.pmtiles");

        let result = backend.read(0, 100).await;
        assert!(matches!(
            result.unwrap_err(),
            PmtError::ObjectStore(object_store::Error::NotFound { .. })
        ));
    }
}
