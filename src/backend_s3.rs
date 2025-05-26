use bytes::Bytes;
use s3::Bucket;

use crate::async_reader::{AsyncBackend, AsyncPmTilesReader};
use crate::cache::{DirectoryCache, NoCache};
use crate::error::PmtError::ResponseBodyTooLong;
use crate::PmtResult;

impl AsyncPmTilesReader<S3Backend, NoCache> {
    /// Creates a new `PMTiles` reader from a bucket and path to the
    /// archive using the `rust-s3` backend.
    ///
    /// Fails if `bucket` or `path` does not exist or is an invalid archive. (Note: S3 requests are made to validate it.)
    pub async fn new_with_bucket_path(bucket: Bucket, path: String) -> PmtResult<Self> {
        Self::new_with_cached_bucket_path(NoCache, bucket, path).await
    }
}

impl<C: DirectoryCache + Sync + Send> AsyncPmTilesReader<S3Backend, C> {
    /// Creates a new `PMTiles` reader from a bucket and path to the
    /// archive using the `rust-s3` backend with a given `cache` backend.
    ///
    /// Fails if `bucket` or `path` does not exist or is an invalid archive.
    /// Note that S3 requests are made to validate it.
    pub async fn new_with_cached_bucket_path(
        cache: C,
        bucket: Bucket,
        path: String,
    ) -> PmtResult<Self> {
        let backend = S3Backend::from(bucket, path);

        Self::try_from_cached_source(backend, cache).await
    }
}

pub struct S3Backend {
    bucket: Bucket,
    path: String,
}

impl S3Backend {
    #[must_use]
    pub fn from(bucket: Bucket, path: String) -> S3Backend {
        Self { bucket, path }
    }
}

impl AsyncBackend for S3Backend {
    async fn read(&self, offset: usize, length: usize) -> PmtResult<Bytes> {
        let response = self
            .bucket
            .get_object_range(
                self.path.as_str(),
                offset as _,
                Some((offset + length - 1) as _),
            )
            .await?;

        let response_bytes = response.bytes();

        if response_bytes.len() > length {
            Err(ResponseBodyTooLong(response_bytes.len(), length))
        } else {
            Ok(response_bytes.clone())
        }
    }
}
