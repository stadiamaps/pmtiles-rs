use std::path::Path;

use async_trait::async_trait;
use fmmap::tokio::{AsyncMmapFile, AsyncMmapFileExt, AsyncOptions};
use tokio::io::AsyncReadExt;

use crate::async_reader::AsyncBackend;
use crate::Error;

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

#[async_trait]
impl AsyncBackend for MmapBackend {
    async fn read_bytes(&self, dst: &mut [u8], offset: usize) -> Result<(), Error> {
        self.file.reader(offset).unwrap().read_exact(dst).await?;

        Ok(())
    }
}
