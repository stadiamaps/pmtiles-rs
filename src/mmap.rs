use std::path::Path;

use async_trait::async_trait;
use fmmap::tokio::{AsyncMmapFile, AsyncMmapFileExt, AsyncOptions};
use tokio::io::AsyncReadExt;

use crate::{async_reader::AsyncBackend, error::Error};

pub struct MmapBackend {
    file: AsyncMmapFile,
}

impl MmapBackend {
    pub async fn try_from(p: &Path) -> Result<Self, Error> {
        Ok(Self {
            file: AsyncMmapFile::open_with_options(p, AsyncOptions::new().read(true))
                .await
                .map_err(|_| Error::UnableToOpenMmapFile)?,
        })
    }
}

impl From<fmmap::error::Error> for Error {
    fn from(_: fmmap::error::Error) -> Self {
        Self::Reading(std::io::Error::from(std::io::ErrorKind::UnexpectedEof))
    }
}

#[async_trait]
impl AsyncBackend for MmapBackend {
    async fn read_exact(&self, dst: &mut [u8], offset: usize) -> Result<(), Error> {
        self.file.reader(offset)?.read_exact(dst).await?;

        Ok(())
    }

    async fn read(&self, dst: &mut [u8], offset: usize) -> Result<usize, Error> {
        let mut reader = self.file.reader(offset)?;

        let read_length = dst.len().min(reader.len());
        reader.read_exact(&mut dst[..read_length]).await?;

        Ok(read_length)
    }
}
