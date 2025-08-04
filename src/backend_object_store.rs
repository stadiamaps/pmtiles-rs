//! Object store backend implementation using the [`object_store`] crate.
//!
//! This backend provides a unified interface for accessing `PMTiles` from various
//! storage systems including S3, Azure Blob Storage, Google Cloud Storage, local files, HTTP/WebDAV Storage, memory and custom implementations.

use std::ops::Range;

use bytes::Bytes;
use object_store::ObjectStore;
use object_store::path::Path;
use url::Url;

use crate::{AsyncBackend, PmtError, PmtResult};

/// Backend implementation using the [`object_store`] crate for unified storage access.
///
/// This backend can work with any storage system supported by [`object_store`]:
/// - [AWS S3](https://aws.amazon.com/s3/) via `object-store-aws` feature
/// - [Azure Blob Storage](https://azure.microsoft.com/en-us/services/storage/blobs/) via `object-store-azure` feature
/// - [Google Cloud Storage](https://cloud.google.com/storage) via `object-store-gcp` feature
/// - Local files via `object-store-fs` feature
/// - [HTTP/WebDAV Storage](https://datatracker.ietf.org/doc/html/rfc2518) via `object-store-http` feature
/// - (mostly for testing) Memory
/// - Custom implementations
///
/// # Examples
///
/// Creating a backend from a URL using `try_from`:
/// ```rust
/// # use pmtiles::ObjectStoreBackend;
/// # use url::Url;
/// #
/// // Create from any supported URL scheme
/// // For example http  (under object-store-http feature)
/// let url = Url::parse("https://example.com/tiles.pmtiles").unwrap();
/// let backend = ObjectStoreBackend::try_from(&url).unwrap();
/// # assert_eq!(backend.path().as_ref(), "tiles.pmtiles");
/// # assert_eq!(backend.store().to_string(), "HttpStore");
///
/// // Works with S3 URLs too (under object-store-aws feature)
/// let url = Url::parse("s3://bucket-name/path/tiles.pmtiles").unwrap();
/// let backend = ObjectStoreBackend::try_from(&url).unwrap();
/// # assert_eq!(backend.path().as_ref(), "path/tiles.pmtiles");
/// # assert_eq!(backend.store().to_string(), "AmazonS3(bucket-name)");
///
/// // Or with URLs that encode the bucket name in the URL path
/// let url = Url::parse("https://ACCOUNT_ID.r2.cloudflarestorage.com/bucket/path").unwrap();
/// let backend = ObjectStoreBackend::try_from(&url).unwrap();
/// # assert_eq!(backend.path().as_ref(), "path");
/// # assert_eq!(backend.store().to_string(), "AmazonS3(bucket)");
/// ```
///
/// Creating a backend manually:
/// ```rust
/// # #[cfg(feature = "object-store-http")]
/// # {
/// let store = Box::new(
///     object_store::http::HttpBuilder::new()
///         .with_url("https://example.com")
///         .build()
///         .unwrap()
/// );
/// let backend = pmtiles::ObjectStoreBackend::new(store, "tiles.pmtiles");
/// # }
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
    /// ```rust,ignore
    /// # #[cfg(feature = "object-store-http")]
    /// # {
    /// use object_store::http::HttpBuilder;
    /// use pmtiles::ObjectStoreBackend;
    /// use std::sync::Arc;
    ///
    /// let store = Arc::new(
    ///     HttpBuilder::new()
    ///         .with_url("https://example.com")
    ///         .build()
    ///         .unwrap()
    /// );
    /// let backend = ObjectStoreBackend::new(store, "tiles.pmtiles");
    /// # }
    /// ```
    ///
    /// For convenience, you can also create backends directly from URLs:
    /// ```rust,ignore
    /// # #[cfg(feature = "object-store-http")]
    /// # {
    /// use pmtiles::ObjectStoreBackend;
    /// use url::Url;
    ///
    /// let url = Url::parse("https://example.com/tiles.pmtiles").unwrap();
    /// let backend = ObjectStoreBackend::try_from(&url).unwrap();
    /// # }
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

#[cfg(any(
    feature = "object-store-fs",
    feature = "object-store-http",
    feature = "object-store-aws",
    feature = "object-store-azure",
    feature = "object-store-gcp",
))]
impl TryFrom<&Url> for ObjectStoreBackend {
    type Error = PmtError;

    /// Create an [`ObjectStoreBackend`] based on the provided `url`
    ///
    /// The url can be for example:
    /// * `file:///path/to/my/file`
    /// * `s3://bucket/path`
    /// * `https://example.com/path.pmtiles`
    fn try_from(value: &Url) -> Result<Self, Self::Error> {
        let (store, path) = object_store::parse_url(value)?;
        Ok(ObjectStoreBackend { store, path })
    }
}

#[cfg(any(
    feature = "object-store-fs",
    feature = "object-store-http",
    feature = "object-store-aws",
    feature = "object-store-azure",
    feature = "object-store-gcp",
))]
impl<I, K, V> TryFrom<(&Url, I)> for ObjectStoreBackend
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<str>,
    V: Into<String>,
{
    type Error = PmtError;

    /// Create an [`ObjectStoreBackend`] based on the provided `url` and `options`
    ///
    /// The url can be for example:
    /// * `file:///path/to/my/file`
    /// * `s3://bucket/path`
    /// * `https://example.com/path.pmtiles`
    ///
    /// Arguments:
    /// * `url`: The URL to parse
    /// * `options`: A list of key-value pairs to pass to the [`ObjectStore`] builder.
    ///   Note different object stores accept different configuration options, so
    ///   the options that are read depends on the `url` value. One common pattern
    ///   is to pass configuration information via process variables using
    ///   [`std::env::vars`].
    fn try_from((url, options): (&Url, I)) -> Result<Self, Self::Error> {
        let (store, path) = object_store::parse_url_opts(url, options)?;
        Ok(ObjectStoreBackend { store, path })
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
    async fn test_error_conversion() {
        let store = Box::new(InMemory::new());
        let backend = ObjectStoreBackend::new(store, "nonexistent.pmtiles");

        let result = backend.read(0, 100).await;
        assert!(matches!(
            result.unwrap_err(),
            PmtError::ObjectStore(object_store::Error::NotFound { .. })
        ));
    }

    #[tokio::test]
    async fn basic_http_test() {
        let url_http =
            Url::parse("https://protomaps.github.io/PMTiles/protomaps(vector)ODbL_firenze.pmtiles")
                .unwrap();
        let backend = ObjectStoreBackend::try_from(&url_http).unwrap();
        assert_eq!(
            backend.path().as_ref(),
            "PMTiles/protomaps(vector)ODbL_firenze.pmtiles"
        );
        assert_eq!(backend.store().to_string(), "HttpStore");
    }

    #[tokio::test]
    #[cfg(feature = "object-store-http")]
    async fn test_try_from_with_http_options() {
        use httpmock::MockServer;

        let mock = MockServer::start();
        let server = mock.mock(|when, then| {
            when.path("/foo/bar")
                .and(|when| when.header("User-Agent", "pmties"));
            then.status(200);
        });
        let url = mock.url("/foo/bar").parse().unwrap();
        dbg!(mock.url("/foo/bar"));

        let opts = [("user_agent", "pmties"), ("allow_http", "true")];
        let backend = ObjectStoreBackend::try_from((&url, opts)).unwrap();
        assert_eq!(backend.path().as_ref(), "foo/bar");
        backend.store().get(backend.path()).await.unwrap();

        server.assert_hits(1);
    }
}
