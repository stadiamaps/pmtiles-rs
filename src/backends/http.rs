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
        Self::new_with_cached_url(NoCache, client, url).await
    }
}

impl<C: DirectoryCache + Sync + Send> AsyncPmTilesReader<HttpBackend, C> {
    /// Creates a new `PMTiles` reader with cache from a URL using the Reqwest backend.
    ///
    /// Fails if `url` does not exist or is an invalid archive. (Note: HTTP requests are made to validate it.)
    ///
    /// # Errors
    ///
    /// This function will return an error if the
    /// - URL is invalid,
    /// - the backend fails to read the header/root directory,
    /// - or if the root directory is malformed
    pub async fn new_with_cached_url<U: IntoUrl>(
        cache: C,
        client: Client,
        url: U,
    ) -> PmtResult<Self> {
        let backend = HttpBackend::try_from(client, url)?;

        Self::try_from_cached_source(backend, cache).await
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
        Ok(HttpBackend {
            client,
            url: url.into_url()?,
        })
    }
}

impl AsyncBackend for HttpBackend {
    async fn read(&self, offset: usize, length: usize) -> PmtResult<BackendResponse> {
        let end = offset + length - 1;
        let range = format!("bytes={offset}-{end}");
        let range = HeaderValue::try_from(range)?;

        let mut req = Request::new(Method::GET, self.url.clone());
        req.headers_mut().insert(RANGE, range);

        let response = self.client.execute(req).await?.error_for_status()?;
        if response.status() != StatusCode::PARTIAL_CONTENT {
            return Err(PmtError::RangeRequestsUnsupported);
        }

        let headers = response.headers();
        let data_version_header_value = headers.get(ETAG).or(headers.get(LAST_MODIFIED));
        let data_version_string = data_version_header_value
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);

        let response_bytes = response.bytes().await?;

        if response_bytes.len() > length {
            Err(PmtError::ResponseBodyTooLong(response_bytes.len(), length))
        } else {
            Ok(match data_version_string {
                Some(v) => BackendResponse::new_with_version(response_bytes, v.to_string()),
                None => BackendResponse::new(response_bytes),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static TEST_URL: &str =
        "https://protomaps.github.io/PMTiles/protomaps(vector)ODbL_firenze.pmtiles";

    #[tokio::test]
    async fn basic_http_test() {
        let client = Client::builder().use_rustls_tls().build().unwrap();
        let backend = HttpBackend::try_from(client, TEST_URL).unwrap();

        AsyncPmTilesReader::try_from_source(backend).await.unwrap();
    }

    async fn make_backend(server: &mockito::Server) -> HttpBackend {
        HttpBackend::try_from(Client::new(), server.url()).expect("valid url")
    }

    #[tokio::test]
    async fn read_no_data_version_header() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("GET", "/")
            .match_header("range", mockito::Matcher::Any)
            .with_status(206)
            .with_body(vec![0u8; 64])
            .create_async()
            .await;

        let backend = make_backend(&server).await;
        let response = backend.read(0, 64).await.expect("read succeeded");
        assert!(response.data_version_string.is_none());
    }

    #[tokio::test]
    async fn read_prefers_etag_over_last_modified() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("GET", "/")
            .match_header("range", mockito::Matcher::Any)
            .with_status(206)
            .with_header("ETag", "\"abc123\"")
            .with_header("Last-Modified", "Wed, 01 Jan 2025 00:00:00 GMT")
            .with_body(vec![0u8; 64])
            .create_async()
            .await;

        let backend = make_backend(&server).await;
        let response = backend.read(0, 64).await.expect("read succeeded");
        assert_eq!(response.data_version_string.as_deref(), Some("\"abc123\""));
    }

    #[tokio::test]
    async fn read_falls_back_to_last_modified() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("GET", "/")
            .match_header("range", mockito::Matcher::Any)
            .with_status(206)
            .with_header("Last-Modified", "Wed, 01 Jan 2025 00:00:00 GMT")
            .with_body(vec![0u8; 64])
            .create_async()
            .await;

        let backend = make_backend(&server).await;
        let response = backend.read(0, 64).await.expect("read succeeded");
        assert_eq!(
            response.data_version_string.as_deref(),
            Some("Wed, 01 Jan 2025 00:00:00 GMT")
        );
    }
}
