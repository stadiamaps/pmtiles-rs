use std::io;
use std::path::Path;

use bytes::Bytes;
use memmap2::Mmap;

use crate::{
    AsyncBackend, AsyncPmTilesReader, BackendResponse, DirectoryCache, NoCache, PmtError, PmtResult,
};

impl AsyncPmTilesReader<MmapBackend, NoCache> {
    /// Creates a new `PMTiles` reader from a file path using the async mmap backend.
    ///
    /// Fails if `path` does not exist or is an invalid archive.
    ///
    /// # Safety of memory mapping
    ///
    /// See [`MmapBackend::try_from`] - the file must not be modified or truncated by another
    /// process while this reader, or any tile data obtained from it, is alive.
    ///
    /// # Errors
    ///
    /// This function will return an error if the
    /// - file cannot be opened for memory mapping,
    /// - backend fails to read the header/root directory or
    /// - root directory is malformed
    pub async fn new_with_path<P: AsRef<Path>>(path: P) -> PmtResult<Self> {
        Self::new_with_cached_path(NoCache, path).await
    }
}

impl<C: DirectoryCache + Sync + Send> AsyncPmTilesReader<MmapBackend, C> {
    /// Creates a new cached `PMTiles` reader from a file path using the async mmap backend.
    ///
    /// Fails if `path` does not exist or is an invalid archive.
    ///
    /// # Safety of memory mapping
    ///
    /// See [`MmapBackend::try_from`] - the file must not be modified or truncated by another
    /// process while this reader, or any tile data obtained from it, is alive.
    ///
    /// # Errors
    ///
    /// This function will return an error if the
    /// - file cannot be opened for memory mapping,
    /// - backend fails to read the header/root directory or
    /// - root directory is malformed
    pub async fn new_with_cached_path<P: AsRef<Path>>(cache: C, path: P) -> PmtResult<Self> {
        let backend = MmapBackend::try_from(path).await?;

        Self::try_from_cached_source(backend, cache).await
    }
}

/// Backend for reading `PMTiles` from a memory-mapped file.
pub struct MmapBackend {
    /// The entire mapping, viewed as [`Bytes`].
    ///
    /// [`Bytes::from_owner`] keeps the underlying [`Mmap`] alive for as long as this value, or
    /// any slice taken from it, is alive. Reads are therefore zero-copy: they only bump a
    /// reference count.
    bytes: Bytes,
}

impl MmapBackend {
    /// Creates a new memory-mapped file backend.
    ///
    /// # Safety of memory mapping
    ///
    /// Memory mapping requires that the file not be modified or truncated by another process for
    /// as long as this backend, or any tile data returned from it, remains alive. If the file
    /// is truncated, reads through the mapping abort the process (`SIGBUS` on Unix, an SEH
    /// exception on Windows). Because tile data borrows directly from the mapping, that
    /// requirement extends to the lifetime of the returned bytes, not just of this backend.
    ///
    /// Use a different backend if the file may change underneath you.
    ///
    /// # Errors
    ///
    /// This function will return an error if the file cannot be opened for memory mapping.
    pub async fn try_from<P: AsRef<Path>>(p: P) -> PmtResult<Self> {
        let file = tokio::fs::File::open(p)
            .await
            .map_err(|_| PmtError::UnableToOpenMmapFile)?
            .into_std()
            .await;

        // SAFETY: Memory mapping a file is inherently unsafe - another process truncating or
        // rewriting the file invalidates the mapped pages, and reading them is undefined
        // behavior. That cannot be enforced from here, so the requirement is documented on
        // this function and on every public constructor that reaches it.
        #[expect(
            unsafe_code,
            reason = "mmap of a file cannot be made safe; the contract is documented on the constructors"
        )]
        let mmap = unsafe { Mmap::map(&file) }.map_err(|_| PmtError::UnableToOpenMmapFile)?;

        Ok(Self {
            bytes: Bytes::from_owner(mmap),
        })
    }
}

/// The error returned when a read runs past the end of the mapping.
fn eof() -> PmtError {
    PmtError::Reading(io::Error::from(io::ErrorKind::UnexpectedEof))
}

impl AsyncBackend for MmapBackend {
    async fn read_exact(&self, offset: usize, length: usize) -> PmtResult<BackendResponse> {
        match offset
            .checked_add(length)
            .filter(|end| *end <= self.bytes.len())
        {
            Some(end) => Ok(BackendResponse::new(self.bytes.slice(offset..end))),
            None => Err(eof()),
        }
    }

    async fn read(&self, offset: usize, length: usize) -> PmtResult<BackendResponse> {
        // An offset at exactly the end of the file is a valid empty read; past it is an error.
        if offset > self.bytes.len() {
            return Err(eof());
        }

        // Clamp to what is actually left *after* `offset`. Short reads are the normal path
        // here: the reader always asks for `MAX_INITIAL_BYTES` up front, which is more than
        // the total size of a small archive.
        let end = offset.saturating_add(length).min(self.bytes.len());

        Ok(BackendResponse::new(self.bytes.slice(offset..end)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::RASTER_FILE;

    async fn backend() -> (MmapBackend, usize) {
        let backend = MmapBackend::try_from(RASTER_FILE).await.unwrap();
        let len = backend.bytes.len();
        assert!(
            len > 100,
            "fixture is too small to exercise the bounds logic"
        );
        (backend, len)
    }

    #[tokio::test]
    async fn read_whole_file() {
        let (backend, len) = backend().await;
        let expected = std::fs::read(RASTER_FILE).unwrap();

        let res = backend.read(0, len).await.unwrap();
        assert_eq!(res.bytes.len(), len);
        assert_eq!(&res.bytes[..], &expected[..]);
        assert!(res.data_version_string.is_none());
    }

    #[tokio::test]
    async fn read_clamps_to_eof_at_offset_zero() {
        let (backend, len) = backend().await;

        let res = backend.read(0, len * 2).await.unwrap();
        assert_eq!(res.bytes.len(), len);
    }

    /// Regression test: clamping must account for `offset`, not just the total length.
    #[tokio::test]
    async fn read_clamps_relative_to_offset() {
        let (backend, len) = backend().await;
        let offset = len - 10;

        let res = backend.read(offset, 100).await.unwrap();
        assert_eq!(res.bytes.len(), 10);

        let expected = std::fs::read(RASTER_FILE).unwrap();
        assert_eq!(&res.bytes[..], &expected[offset..]);
    }

    #[tokio::test]
    async fn read_at_eof_is_empty() {
        let (backend, len) = backend().await;

        let res = backend.read(len, 10).await.unwrap();
        assert!(res.bytes.is_empty());
    }

    #[tokio::test]
    async fn read_past_eof_errors() {
        let (backend, len) = backend().await;

        assert!(matches!(
            backend.read(len + 1, 10).await,
            Err(PmtError::Reading(_))
        ));
    }

    #[tokio::test]
    async fn read_exact_matches_the_file() {
        let (backend, _) = backend().await;
        let expected = std::fs::read(RASTER_FILE).unwrap();

        let res = backend.read_exact(20, 50).await.unwrap();
        assert_eq!(&res.bytes[..], &expected[20..70]);
    }

    #[tokio::test]
    async fn read_exact_past_eof_errors() {
        let (backend, len) = backend().await;

        assert!(matches!(
            backend.read_exact(len - 5, 10).await,
            Err(PmtError::Reading(_))
        ));
    }

    #[tokio::test]
    async fn read_exact_does_not_overflow() {
        let (backend, _) = backend().await;

        assert!(matches!(
            backend.read_exact(usize::MAX, 1).await,
            Err(PmtError::Reading(_))
        ));
    }

    /// Reads must be views into the mapping, not copies of it. Two reads of the same range
    /// return the same address; a copy would allocate a fresh buffer each time.
    #[tokio::test]
    async fn reads_are_zero_copy() {
        let (backend, _) = backend().await;

        let a = backend.read_exact(64, 32).await.unwrap().bytes;
        let b = backend.read_exact(64, 32).await.unwrap().bytes;
        assert_eq!(a.as_ptr(), b.as_ptr());

        // ...and a read at a different offset lands the same distance further into the map.
        let c = backend.read_exact(96, 32).await.unwrap().bytes;
        assert_eq!(c.as_ptr() as usize - a.as_ptr() as usize, 32);
    }

    /// Reads must be independent of each other - there is no shared cursor.
    #[tokio::test]
    async fn reads_are_position_independent() {
        let (backend, _) = backend().await;

        let (a, b) = tokio::join!(backend.read_exact(0, 40), backend.read_exact(60, 40));
        let expected = std::fs::read(RASTER_FILE).unwrap();

        assert_eq!(&a.unwrap().bytes[..], &expected[0..40]);
        assert_eq!(&b.unwrap().bytes[..], &expected[60..100]);
    }
}
