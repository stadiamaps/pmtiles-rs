use reqwest::header::{ETAG, HeaderValue, LAST_MODIFIED, RANGE};
use reqwest::{Client, IntoUrl, Method, Request, StatusCode, Url};

use crate::{
    AsyncBackend, AsyncPmTilesReader, BackendResponse, DirectoryCache, NoCache, PmtError, PmtResult,
};

impl AsyncPmTilesReader<HttpBackend, NoCache> {
    /// Creates a new `PMTiles` reader from a URL using the Reqwest backend.
    ///
    /// Fails if `url` does not exist or is an invalid archive. (Note: HTTP requests are made to validate it.)
    ///
    /// # Errors
    ///
    /// This function will return an error if the
    /// - URL is invalid,
    /// - the backend fails to read the header/root directory,
    /// - or if the root directory is malformed
    pub async fn new_with_url<U: IntoUrl>(client: Client, url: U) -> PmtResult<Self> {
        Self::new_with_url_and_cache(client, url, NoCache).await
    }
}

impl<C: DirectoryCache + Sync + Send> AsyncPmTilesReader<HttpBackend, C> {
    /// Creates a new `PMTiles` reader with a directory cache from a URL using the Reqwest backend.
    ///
    /// Fails if `url` does not exist or is an invalid archive. (Note: HTTP requests are made to validate it.)
    ///
    /// # Errors
    ///
    /// This function will return an error if the
    /// - URL is invalid,
    /// - the backend fails to read the header/root directory,
    /// - or if the root directory is malformed
    pub async fn new_with_url_and_cache<U: IntoUrl>(
        client: Client,
        url: U,
        cache: C,
    ) -> PmtResult<Self> {
        let backend = HttpBackend::try_from(client, url)?;

        Self::try_from_source_and_cache(backend, cache).await
    }

    /// Creates a new `PMTiles` reader with a directory cache from a URL using the Reqwest backend.
    ///
    /// # Errors
    ///
    /// See [`Self::new_with_url_and_cache`].
    #[deprecated(
        since = "0.27.0",
        note = "renamed to `new_with_url_and_cache`, which takes the cache last"
    )]
    pub async fn new_with_cached_url<U: IntoUrl>(
        cache: C,
        client: Client,
        url: U,
    ) -> PmtResult<Self> {
        Self::new_with_url_and_cache(client, url, cache).await
    }
}

/// Backend for reading `PMTiles` over HTTP.
pub struct HttpBackend {
    client: Client,
    url: Url,
}

impl HttpBackend {
    /// Creates a new HTTP backend.
    ///
    /// # Errors
    ///
    /// This function will return an error if the URL cannot be parsed into a valid URL.
    pub fn try_from<U: IntoUrl>(client: Client, url: U) -> PmtResult<Self> {
        Ok(Self {
            client,
            url: url.into_url()?,
        })
    }
}

impl AsyncBackend for HttpBackend {
    async fn read(&self, offset: u64, length: usize) -> PmtResult<BackendResponse> {
        let end = offset + length as u64 - 1;
        let range = format!("bytes={offset}-{end}");
        let range = HeaderValue::try_from(range)?;

        let mut req = Request::new(Method::GET, self.url.clone());
        req.headers_mut().insert(RANGE, range);

        let response = self.client.execute(req).await?.error_for_status()?;
        if response.status() != StatusCode::PARTIAL_CONTENT {
            return Err(PmtError::RangeRequestsUnsupported);
        }

        let headers = response.headers();
        let data_version_string = headers
            .get(ETAG)
            .or_else(|| headers.get(LAST_MODIFIED))
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);

        Ok(BackendResponse {
            bytes: response.bytes().await?,
            data_version_string,
        })
    }
}

#[cfg(test)]
mod tests {
    use mockito::{Matcher, Server};

    use super::*;
    use crate::backends::test_server::serve_file;
    use crate::tests::RASTER_FILE;
    use crate::{HashMapCache, TileCoord};

    #[tokio::test]
    async fn reader_reads_tiles() {
        let mut server = Server::new_async().await;
        let _mock = serve_file(&mut server, RASTER_FILE, &[]).await;
        let tile = TileCoord::new(3, 4, 2).unwrap();

        let reader = AsyncPmTilesReader::new_with_url(Client::new(), server.url())
            .await
            .unwrap();
        assert!(reader.get_tile(tile).await.unwrap().is_some());

        let reader = AsyncPmTilesReader::new_with_url_and_cache(
            Client::new(),
            server.url(),
            HashMapCache::default(),
        )
        .await
        .unwrap();
        assert!(reader.get_tile(tile).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn read_requires_range_support() {
        let mut server = Server::new_async().await;
        let _mock = server
            .mock("GET", "/")
            .with_status(200)
            .with_body(vec![0u8; 64])
            .create_async()
            .await;

        let result = make_backend(&server).read(0, 64).await;
        assert!(matches!(result, Err(PmtError::RangeRequestsUnsupported)));
    }

    fn make_backend(server: &Server) -> HttpBackend {
        HttpBackend::try_from(Client::new(), server.url()).expect("valid url")
    }

    #[tokio::test]
    async fn read_no_data_version_header() {
        let mut server = Server::new_async().await;
        let _mock = server
            .mock("GET", "/")
            .match_header("range", Matcher::Any)
            .with_status(206)
            .with_body(vec![0u8; 64])
            .create_async()
            .await;

        let backend = make_backend(&server);
        let response = backend.read(0, 64).await.expect("read succeeded");
        assert!(response.data_version_string.is_none());
    }

    #[tokio::test]
    async fn read_prefers_etag_over_last_modified() {
        let mut server = Server::new_async().await;
        let _mock = server
            .mock("GET", "/")
            .match_header("range", Matcher::Any)
            .with_status(206)
            .with_header("ETag", "\"abc123\"")
            .with_header("Last-Modified", "Wed, 01 Jan 2025 00:00:00 GMT")
            .with_body(vec![0u8; 64])
            .create_async()
            .await;

        let backend = make_backend(&server);
        let response = backend.read(0, 64).await.expect("read succeeded");
        assert_eq!(response.data_version_string.as_deref(), Some("\"abc123\""));
    }

    #[tokio::test]
    async fn read_falls_back_to_last_modified() {
        let mut server = Server::new_async().await;
        let _mock = server
            .mock("GET", "/")
            .match_header("range", Matcher::Any)
            .with_status(206)
            .with_header("Last-Modified", "Wed, 01 Jan 2025 00:00:00 GMT")
            .with_body(vec![0u8; 64])
            .create_async()
            .await;

        let backend = make_backend(&server);
        let response = backend.read(0, 64).await.expect("read succeeded");
        assert_eq!(
            response.data_version_string.as_deref(),
            Some("Wed, 01 Jan 2025 00:00:00 GMT")
        );
    }
}
