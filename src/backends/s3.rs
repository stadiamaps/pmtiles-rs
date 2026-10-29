use s3::Bucket;

use crate::PmtError::ResponseBodyTooLong;
use crate::{
    AsyncBackend, AsyncPmTilesReader, BackendResponse, DirectoryCache, NoCache, PmtResult,
};

impl AsyncPmTilesReader<S3Backend, NoCache> {
    /// Creates a new `PMTiles` reader from a bucket and path to the
    /// archive using the `rust-s3` backend.
    ///
    /// Fails if `bucket` or `path` does not exist or is an invalid archive. (Note: S3 requests are made to validate it.)
    ///
    /// # Errors
    ///
    /// This function will return an error if the
    /// - the backend fails to read the header/root directory,
    /// - or if the root directory is malformed
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
    ///
    /// # Errors
    ///
    /// This function will return an error if the
    /// - the backend fails to read the header/root directory,
    /// - or if the root directory is malformed
    pub async fn new_with_cached_bucket_path(
        cache: C,
        bucket: Bucket,
        path: String,
    ) -> PmtResult<Self> {
        let backend = S3Backend::from(bucket, path);

        Self::try_from_cached_source(backend, cache).await
    }
}

/// Backend for reading `PMTiles` from S3.
pub struct S3Backend {
    bucket: Bucket,
    path: String,
}

impl S3Backend {
    #[must_use]
    /// Creates a new S3 backend.
    pub fn from(bucket: Bucket, path: String) -> S3Backend {
        Self { bucket, path }
    }
}

impl AsyncBackend for S3Backend {
    async fn read(&self, offset: u64, length: u64) -> PmtResult<BackendResponse> {
        let response = self
            .bucket
            .get_object_range(self.path.as_str(), offset, Some(offset + length - 1))
            .await?;

        let response_bytes = response.bytes();

        if response_bytes.len() as u64 > length {
            Err(ResponseBodyTooLong(response_bytes.len() as u64, length))
        } else {
            let headers = response.headers();
            let data_version = headers
                .get("etag")
                .or_else(|| headers.get("last-modified"))
                .cloned();

            Ok(match data_version {
                Some(v) => BackendResponse::new_with_version(response_bytes.clone(), v),
                None => BackendResponse::new(response_bytes.clone()),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use s3::Region;
    use s3::creds::Credentials;

    use super::*;
    use crate::backends::test_server::serve_file;
    use crate::tests::RASTER_FILE;
    use crate::{HashMapCache, TileCoord};

    fn bucket(server: &mockito::ServerGuard) -> Bucket {
        let region = Region::Custom {
            region: "us-east-1".to_owned(),
            endpoint: server.url(),
        };
        let bucket = Bucket::new("tiles", region, Credentials::anonymous().unwrap()).unwrap();
        *bucket.with_path_style()
    }

    #[tokio::test]
    async fn reader_reads_tiles() {
        let mut server = mockito::Server::new_async().await;
        let _mock = serve_file(&mut server, RASTER_FILE, &[]).await;
        let tile = TileCoord::new(3, 4, 2).unwrap();

        let reader = AsyncPmTilesReader::new_with_bucket_path(bucket(&server), "a.pmtiles".into())
            .await
            .unwrap();
        assert!(reader.get_tile(tile).await.unwrap().is_some());

        let reader = AsyncPmTilesReader::new_with_cached_bucket_path(
            HashMapCache::default(),
            bucket(&server),
            "a.pmtiles".into(),
        )
        .await
        .unwrap();
        assert!(reader.get_tile(tile).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn read_without_data_version() {
        let mut server = mockito::Server::new_async().await;
        let _mock = serve_file(&mut server, RASTER_FILE, &[]).await;

        let backend = S3Backend::from(bucket(&server), "a.pmtiles".into());
        let response = backend.read(0, 64).await.unwrap();
        assert_eq!(response.bytes.len(), 64);
        assert!(response.data_version_string.is_none());
    }

    #[tokio::test]
    async fn read_prefers_etag_over_last_modified() {
        let mut server = mockito::Server::new_async().await;
        let headers = [
            ("ETag", "\"abc123\""),
            ("Last-Modified", "Wed, 01 Jan 2025 00:00:00 GMT"),
        ];
        let _mock = serve_file(&mut server, RASTER_FILE, &headers).await;

        let backend = S3Backend::from(bucket(&server), "a.pmtiles".into());
        let response = backend.read(0, 64).await.unwrap();
        assert_eq!(response.data_version_string.as_deref(), Some("\"abc123\""));
    }

    #[tokio::test]
    async fn read_falls_back_to_last_modified() {
        let mut server = mockito::Server::new_async().await;
        let headers = [("Last-Modified", "Wed, 01 Jan 2025 00:00:00 GMT")];
        let _mock = serve_file(&mut server, RASTER_FILE, &headers).await;

        let backend = S3Backend::from(bucket(&server), "a.pmtiles".into());
        let response = backend.read(0, 64).await.unwrap();
        assert_eq!(
            response.data_version_string.as_deref(),
            Some("Wed, 01 Jan 2025 00:00:00 GMT")
        );
    }

    #[tokio::test]
    async fn read_rejects_too_long_body() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("GET", mockito::Matcher::Any)
            .with_status(206)
            .with_body(vec![0u8; 65])
            .create_async()
            .await;

        let backend = S3Backend::from(bucket(&server), "a.pmtiles".into());
        let result = backend.read(0, 64).await;
        assert!(matches!(result, Err(ResponseBodyTooLong(65, 64))));
    }
}
