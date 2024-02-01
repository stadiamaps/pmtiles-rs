use async_trait::async_trait;
use bytes::Bytes;
use s3::Bucket;

use crate::{
    async_reader::AsyncBackend,
    error::PmtError::{ResponseBodyTooLong, UnexpectedNumberOfBytesReturned},
};

pub struct S3Backend {
    bucket: Bucket,
    pmtiles_path: String,
}

impl S3Backend {
    #[must_use]
    pub fn from(bucket: Bucket, pmtiles_path: String) -> S3Backend {
        Self {
            bucket,
            pmtiles_path,
        }
    }
}

#[async_trait]
impl AsyncBackend for S3Backend {
    async fn read_exact(&self, offset: usize, length: usize) -> crate::error::PmtResult<Bytes> {
        let data = self.read(offset, length).await?;

        if data.len() == length {
            Ok(data)
        } else {
            Err(UnexpectedNumberOfBytesReturned(length, data.len()))
        }
    }

    async fn read(&self, offset: usize, length: usize) -> crate::error::PmtResult<Bytes> {
        let response = self
            .bucket
            .get_object_range(
                self.pmtiles_path.as_str(),
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
