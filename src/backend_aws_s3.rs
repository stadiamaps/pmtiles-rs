use aws_sdk_s3::Client;
use bytes::Bytes;

use crate::async_reader::{AsyncBackend, AsyncPmTilesReader};
use crate::cache::{DirectoryCache, NoCache};
use crate::{PmtError, PmtResult};

impl AsyncPmTilesReader<AwsS3Backend, NoCache> {
    /// Creates a new `PMTiles` reader from a client, bucket and key to the
    /// archive using the `aws-sdk-s3` backend.
    ///
    /// Fails if the `bucket` or `key` does not exist or is an invalid
    /// archive. Note that S3 requests are made to validate it.
    pub async fn new_with_client_bucket_and_path(
        client: Client,
        bucket: String,
        key: String,
    ) -> PmtResult<Self> {
        Self::new_with_cached_client_bucket_and_path(NoCache, client, bucket, key).await
    }
}

impl<C: DirectoryCache + Sync + Send> AsyncPmTilesReader<AwsS3Backend, C> {
    /// Creates a new `PMTiles` reader from a client, bucket and key to the
    /// archive using the `aws-sdk-s3` backend. Caches using the designated
    /// `cache`.
    ///
    /// Fails if the `bucket` or `key` does not exist or is an invalid
    /// archive.
    /// (Note: S3 requests are made to validate it.)
    pub async fn new_with_cached_client_bucket_and_path(
        cache: C,
        client: Client,
        bucket: String,
        key: String,
    ) -> PmtResult<Self> {
        let backend = AwsS3Backend::from(client, bucket, key);

        Self::try_from_cached_source(backend, cache).await
    }
}

pub struct AwsS3Backend {
    client: Client,
    bucket: String,
    key: String,
}

impl AwsS3Backend {
    #[must_use]
    pub fn from(client: Client, bucket: String, key: String) -> Self {
        Self {
            client,
            bucket,
            key,
        }
    }
}

impl AsyncBackend for AwsS3Backend {
    async fn read(&self, offset: usize, length: usize) -> PmtResult<Bytes> {
        let range_end = offset + length - 1;
        let range = format!("bytes={offset}-{range_end}");

        let obj = self
            .client
            .get_object()
            .bucket(self.bucket.clone())
            .key(self.key.clone())
            .range(range)
            .send()
            .await
            .map_err(Box::new)?;

        let response_bytes = obj
            .body
            .collect()
            .await
            .map_err(|e| PmtError::Reading(e.into()))?
            .into_bytes();

        if response_bytes.len() > length {
            Err(PmtError::ResponseBodyTooLong(response_bytes.len(), length))
        } else {
            Ok(response_bytes)
        }
    }
}
