use aws_sdk_s3::Client;

use crate::{
    AsyncBackend, AsyncPmTilesReader, BackendResponse, DirectoryCache, NoCache, PmtError, PmtResult,
};

impl AsyncPmTilesReader<AwsS3Backend, NoCache> {
    /// Creates a new `PMTiles` reader from a client, bucket and key to the
    /// archive using the `aws-sdk-s3` backend.
    ///
    /// Fails if the `bucket` or `key` does not exist or is an invalid
    /// archive. Note that S3 requests are made to validate it.
    ///
    /// # Errors
    ///
    /// This function will return an error if the S3 object does not exist,
    /// the S3 request fails, or if the data is not a valid `PMTiles` archive.
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
    ///
    /// # Errors
    ///
    /// This function will return an error if the S3 object does not exist,
    /// the S3 request fails, or if the data is not a valid `PMTiles` archive.
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

/// Backend for reading `PMTiles` from AWS S3.
pub struct AwsS3Backend {
    client: Client,
    bucket: String,
    key: String,
}

impl AwsS3Backend {
    #[must_use]
    /// Creates a new AWS S3 backend.
    pub fn from(client: Client, bucket: String, key: String) -> Self {
        Self {
            client,
            bucket,
            key,
        }
    }
}

impl AsyncBackend for AwsS3Backend {
    async fn read(&self, offset: u64, length: usize) -> PmtResult<BackendResponse> {
        let range_end = offset + length as u64 - 1;
        let range = format!("bytes={offset}-{range_end}");

        let mut obj = self
            .client
            .get_object()
            .bucket(self.bucket.clone())
            .key(self.key.clone())
            .range(range)
            .send()
            .await
            .map_err(Box::new)?;

        let data_version = obj
            .e_tag
            .take()
            .or_else(|| obj.last_modified.as_ref().map(ToString::to_string));

        let response_bytes = obj
            .body
            .collect()
            .await
            .map_err(|e| PmtError::Reading(e.into()))?
            .into_bytes();

        if response_bytes.len() > length {
            Err(PmtError::ResponseBodyTooLong(response_bytes.len(), length))
        } else {
            Ok(match data_version {
                Some(v) => BackendResponse::new_with_version(response_bytes, v),
                None => BackendResponse::new(response_bytes),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use aws_sdk_s3::config::{BehaviorVersion, Credentials, Region};
    use mockito::{Matcher, Server, ServerGuard};

    use super::*;
    use crate::backends::test_server::serve_file;
    use crate::tests::RASTER_FILE;
    use crate::{HashMapCache, TileCoord};

    fn client(server: &ServerGuard) -> Client {
        let config = aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new("us-east-1"))
            .endpoint_url(server.url())
            .force_path_style(true)
            .credentials_provider(Credentials::new("key", "secret", None, None, "test"))
            .build();
        Client::from_conf(config)
    }

    fn backend(server: &ServerGuard) -> AwsS3Backend {
        AwsS3Backend::from(client(server), "tiles".into(), "a.pmtiles".into())
    }

    #[tokio::test]
    async fn reader_reads_tiles() {
        let mut server = Server::new_async().await;
        let _mock = serve_file(&mut server, RASTER_FILE, &[]).await;
        let tile = TileCoord::new(3, 4, 2).unwrap();

        let reader = AsyncPmTilesReader::new_with_client_bucket_and_path(
            client(&server),
            "tiles".into(),
            "a.pmtiles".into(),
        )
        .await
        .unwrap();
        assert!(reader.get_tile(tile).await.unwrap().is_some());

        let reader = AsyncPmTilesReader::new_with_cached_client_bucket_and_path(
            HashMapCache::default(),
            client(&server),
            "tiles".into(),
            "a.pmtiles".into(),
        )
        .await
        .unwrap();
        assert!(reader.get_tile(tile).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn read_without_data_version() {
        let mut server = Server::new_async().await;
        let _mock = serve_file(&mut server, RASTER_FILE, &[]).await;

        let response = backend(&server).read(0, 64).await.unwrap();
        assert_eq!(response.bytes.len(), 64);
        assert!(response.data_version_string.is_none());
    }

    #[tokio::test]
    async fn read_prefers_etag_over_last_modified() {
        let mut server = Server::new_async().await;
        let headers = [
            ("ETag", "\"abc123\""),
            ("Last-Modified", "Wed, 01 Jan 2025 00:00:00 GMT"),
        ];
        let _mock = serve_file(&mut server, RASTER_FILE, &headers).await;

        let response = backend(&server).read(0, 64).await.unwrap();
        assert_eq!(response.data_version_string.as_deref(), Some("\"abc123\""));
    }

    #[tokio::test]
    async fn read_falls_back_to_last_modified() {
        let mut server = Server::new_async().await;
        let headers = [("Last-Modified", "Wed, 01 Jan 2025 00:00:00 GMT")];
        let _mock = serve_file(&mut server, RASTER_FILE, &headers).await;

        let response = backend(&server).read(0, 64).await.unwrap();
        assert_eq!(
            response.data_version_string.as_deref(),
            Some("2025-01-01T00:00:00Z")
        );
    }

    #[tokio::test]
    async fn read_rejects_too_long_body() {
        let mut server = Server::new_async().await;
        let _mock = server
            .mock("GET", Matcher::Any)
            .with_status(206)
            .with_body(vec![0u8; 65])
            .create_async()
            .await;

        let result = backend(&server).read(0, 64).await;
        assert!(matches!(result, Err(PmtError::ResponseBodyTooLong(65, 64))));
    }

    #[tokio::test]
    async fn read_reports_request_errors() {
        let mut server = Server::new_async().await;
        let _mock = server
            .mock("GET", Matcher::Any)
            .with_status(404)
            .create_async()
            .await;

        assert!(backend(&server).read(0, 64).await.is_err());
    }
}
