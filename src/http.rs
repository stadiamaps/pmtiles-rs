use async_trait::async_trait;
use bytes::Bytes;
use reqwest::header::{HeaderValue, ACCEPT_RANGES, RANGE};
use reqwest::{Client, IntoUrl, Method, Request, Url};

use crate::async_reader::AsyncBackend;
use crate::error::{Error, HttpError};

pub struct HttpBackend {
    client: Client,
    pmtiles_url: Url,
}

impl HttpBackend {
    pub fn try_from<U: IntoUrl>(client: Client, url: U) -> Result<Self, Error> {
        Ok(HttpBackend {
            client,
            pmtiles_url: url.into_url()?,
        })
    }
}

static VALID_ACCEPT_RANGES: HeaderValue = HeaderValue::from_static("bytes");

#[async_trait]
impl AsyncBackend for HttpBackend {
    async fn read_exact(&self, offset: usize, length: usize) -> Result<Bytes, Error> {
        let data = self.read(offset, length).await?;

        if data.len() == length {
            Ok(data)
        } else {
            Err(HttpError::UnexpectedNumberOfBytesReturned(length, data.len()).into())
        }
    }

    async fn read(&self, offset: usize, length: usize) -> Result<Bytes, Error> {
        let mut req = Request::new(Method::GET, self.pmtiles_url.clone());
        let range_header = req
            .headers_mut()
            .entry(RANGE)
            .or_insert(HeaderValue::from_static(""));
        let end = offset + length - 1;
        // This .unwrap() should be safe, since `offset` and `end` will always be valid.
        *range_header = HeaderValue::from_str(format!("bytes={offset}-{end}").as_str()).unwrap();

        let response = self.client.execute(req).await?.error_for_status()?;

        if response.headers().get(ACCEPT_RANGES) != Some(&VALID_ACCEPT_RANGES) {
            return Err(HttpError::RangeRequestsUnsupported.into());
        }

        let response_bytes = response.bytes().await?;

        if response_bytes.len() > length {
            Err(HttpError::ResponseBodyTooLong(response_bytes.len(), length).into())
        } else {
            Ok(response_bytes)
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::async_reader::AsyncPmTilesReader;
    use crate::http::HttpBackend;

    static TEST_URL: &str =
        "https://protomaps.github.io/PMTiles/protomaps(vector)ODbL_firenze.pmtiles";

    #[tokio::test]
    async fn basic_http_test() {
        let client = reqwest::Client::builder().use_rustls_tls().build().unwrap();
        let backend = HttpBackend::try_from(client, TEST_URL).unwrap();

        let _tiles = AsyncPmTilesReader::try_from_source(backend).await.unwrap();
    }
}
