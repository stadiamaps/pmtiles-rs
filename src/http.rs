use async_trait::async_trait;
use bytes::Bytes;
use reqwest::{
    header::{HeaderValue, RANGE},
    Client, IntoUrl, Method, Request, StatusCode, Url,
};

use crate::{
    async_reader::AsyncBackend,
    error::{PmtHttpError, PmtResult},
};

pub struct HttpBackend {
    client: Client,
    pmtiles_url: Url,
}

impl HttpBackend {
    pub fn try_from<U: IntoUrl>(client: Client, url: U) -> PmtResult<Self> {
        Ok(HttpBackend {
            client,
            pmtiles_url: url.into_url()?,
        })
    }
}

#[async_trait]
impl AsyncBackend for HttpBackend {
    async fn read_exact(&self, offset: usize, length: usize) -> PmtResult<Bytes> {
        let data = self.read(offset, length).await?;

        if data.len() == length {
            Ok(data)
        } else {
            Err(PmtHttpError::UnexpectedNumberOfBytesReturned(length, data.len()).into())
        }
    }

    async fn read(&self, offset: usize, length: usize) -> PmtResult<Bytes> {
        let end = offset + length - 1;
        let range = format!("bytes={offset}-{end}");
        let range = HeaderValue::try_from(range).map_err(PmtHttpError::from)?;

        let mut req = Request::new(Method::GET, self.pmtiles_url.clone());
        req.headers_mut().insert(RANGE, range);

        let response = self.client.execute(req).await?.error_for_status()?;
        if response.status() != StatusCode::PARTIAL_CONTENT {
            return Err(PmtHttpError::RangeRequestsUnsupported.into());
        }

        let response_bytes = response.bytes().await?;
        if response_bytes.len() > length {
            Err(PmtHttpError::ResponseBodyTooLong(response_bytes.len(), length).into())
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

        AsyncPmTilesReader::try_from_source(backend).await.unwrap();
    }
}
