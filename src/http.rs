use async_trait::async_trait;
use reqwest::header::{HeaderValue, ACCEPT_RANGES, RANGE};
use reqwest::{Client, IntoUrl, Method, Request, Url};

use crate::{AsyncBackend, Error};

pub struct HttpBackend {
    client: Client,
    pmtiles_url: Url,
}

impl HttpBackend {
    pub fn new<U: IntoUrl>(client: Client, url: U) -> Result<Self, Error> {
        Ok(HttpBackend {
            client,
            pmtiles_url: url.into_url()?,
        })
    }
}

static VALID_ACCEPT_RANGES: HeaderValue = HeaderValue::from_static("bytes");

#[async_trait]
impl AsyncBackend for HttpBackend {
    async fn read_bytes(&self, dst: &mut [u8], offset: usize) -> Result<(), Error> {
        let mut req = Request::new(Method::GET, self.pmtiles_url.clone());
        let range_header = req
            .headers_mut()
            .entry(RANGE)
            .or_insert(HeaderValue::from_static(""));
        let end = offset + dst.len() - 1;
        // This .unwrap() should be safe, since `offset` and `end` will always be valid.
        *range_header = HeaderValue::from_str(format!("bytes={offset}-{end}").as_str()).unwrap();

        let response = self.client.execute(req).await?.error_for_status()?;

        if response.headers().get(ACCEPT_RANGES) != Some(&VALID_ACCEPT_RANGES) {
            return Err(Error::Http("Range requests unsupported".to_string()));
        }

        let response_bytes = response.bytes().await?;
        dst.copy_from_slice(&response_bytes[..]);

        Ok(())
    }
}

impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        Error::Http(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use crate::http::HttpBackend;
    use crate::AsyncPmTilesReader;

    static TEST_URL: &str =
        "https://protomaps.github.io/PMTiles/protomaps(vector)ODbL_firenze.pmtiles";

    #[tokio::test]
    async fn basic_http_test() {
        let client = reqwest::Client::builder()
            .use_rustls_tls()
            .build()
            .expect("Unable to create HTTP client.");
        let backend = HttpBackend::new(client, TEST_URL).expect("Unable to build HTTP backend.");

        let _tiles = AsyncPmTilesReader::try_from_source(backend)
            .await
            .expect("Unable to init PMTiles archive");
    }
}
