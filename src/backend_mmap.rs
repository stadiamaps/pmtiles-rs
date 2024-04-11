use std::io;
use std::path::Path;

use bytes::{Buf, Bytes};
use fmmap::tokio::{AsyncMmapFile, AsyncMmapFileExt as _, AsyncOptions};

use crate::async_reader::{AsyncBackend, AsyncPmTilesReader};
use crate::cache::{DirectoryCache, NoCache};
use crate::error::{PmtError, PmtResult};

impl AsyncPmTilesReader<MmapBackend, NoCache> {
    /// Creates a new `PMTiles` reader from a file path using the async mmap backend.
    ///
    /// Fails if [p] does not exist or is an invalid archive.
    pub async fn new_with_path<P: AsRef<Path>>(path: P) -> PmtResult<Self> {
        Self::new_with_cached_path(NoCache, path).await
    }
}

impl<C: DirectoryCache + Sync + Send> AsyncPmTilesReader<MmapBackend, C> {
    /// Creates a new cached `PMTiles` reader from a file path using the async mmap backend.
    ///
    /// Fails if [p] does not exist or is an invalid archive.
    pub async fn new_with_cached_path<P: AsRef<Path>>(cache: C, path: P) -> PmtResult<Self> {
        let backend = MmapBackend::try_from(path).await?;

        Self::try_from_cached_source(backend, cache).await
    }
}

pub struct MmapBackend {
    file: AsyncMmapFile,
}

impl MmapBackend {
    pub async fn try_from<P: AsRef<Path>>(p: P) -> PmtResult<Self> {
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

impl AsyncBackend for MmapBackend {
    async fn read_exact(&self, offset: usize, length: usize) -> PmtResult<Bytes> {
        if self.file.len() >= offset + length {
            Ok(self.file.reader(offset)?.copy_to_bytes(length))
        } else {
            Err(PmtError::Reading(io::Error::from(
                io::ErrorKind::UnexpectedEof,
            )))
        }
    }

    async fn read(&self, offset: usize, length: usize) -> PmtResult<Bytes> {
        let reader = self.file.reader(offset)?;

        let read_length = length.min(reader.len());

        Ok(self.file.reader(offset)?.copy_to_bytes(read_length))
    }
}
