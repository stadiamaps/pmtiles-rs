use bytes::Bytes;
use reqwest::header::{HeaderValue, RANGE};
use reqwest::{Client, IntoUrl, Method, Request, StatusCode, Url};

use crate::{AsyncBackend, AsyncPmTilesReader, DirectoryCache, NoCache, PmtError, PmtResult};

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
    async fn read(&self, offset: usize, length: usize) -> PmtResult<Bytes> {
        let end = offset + length - 1;
        let range = format!("bytes={offset}-{end}");
        let range = HeaderValue::try_from(range)?;

        let mut req = Request::new(Method::GET, self.url.clone());
        req.headers_mut().insert(RANGE, range);

        let response = self.client.execute(req).await?.error_for_status()?;
        if response.status() != StatusCode::PARTIAL_CONTENT {
            return Err(PmtError::RangeRequestsUnsupported);
        }

        let response_bytes = response.bytes().await?;
        if response_bytes.len() > length {
            Err(PmtError::ResponseBodyTooLong(response_bytes.len(), length))
        } else {
            Ok(response_bytes)
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
}
