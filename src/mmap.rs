use std::io;
use std::path::Path;

use async_trait::async_trait;
use bytes::{Buf, Bytes};
use fmmap::tokio::{AsyncMmapFile, AsyncMmapFileExt as _, AsyncOptions};

use crate::async_reader::AsyncBackend;
use crate::error::PmtError;

pub struct MmapBackend {
    file: AsyncMmapFile,
}

impl MmapBackend {
    pub async fn try_from<P: AsRef<Path>>(p: P) -> Result<Self, PmtError> {
        Ok(Self {
            file: AsyncMmapFile::open_with_options(p, AsyncOptions::new().read(true))
                .await
                .map_err(|_| PmtError::UnableToOpenMmapFile)?,
        })
    }
}

impl From<fmmap::error::Error> for PmtError {
    fn from(_: fmmap::error::Error) -> Self {
        Self::Reading(io::Error::from(io::ErrorKind::UnexpectedEof))
    }
}

#[async_trait]
impl AsyncBackend for MmapBackend {
    async fn read_exact(&self, offset: usize, length: usize) -> Result<Bytes, PmtError> {
        if self.file.len() >= offset + length {
            Ok(self.file.reader(offset)?.copy_to_bytes(length))
        } else {
            Err(PmtError::Reading(io::Error::from(
                io::ErrorKind::UnexpectedEof,
            )))
        }
    }

    async fn read(&self, offset: usize, length: usize) -> Result<Bytes, PmtError> {
        let reader = self.file.reader(offset)?;

        let read_length = length.min(reader.len());

        Ok(self.file.reader(offset)?.copy_to_bytes(read_length))
    }
}
