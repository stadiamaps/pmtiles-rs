#[cfg(any(unix, windows))]
use std::fs::File;
use std::future::{self, Future};
#[cfg(feature = "iter")]
use std::iter;
#[cfg(any(unix, windows))]
use std::path::Path;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};

use bytes::Bytes;
#[cfg(feature = "iter")]
use futures_util::StreamExt as _;
#[cfg(feature = "tilejson")]
use tilejson::TileJSON;

#[cfg(feature = "iter")]
use crate::DirEntry;
use crate::async_reader::read_bytes;
#[cfg(any(unix, windows))]
use crate::async_reader::{read_full_at, read_options};
use crate::{
    AsyncBackend, AsyncPmTilesReader, BackendResponse, DirectoryCache, Header, NoCache, PmtResult,
    TileId,
};

/// A trait for synchronous backends that can read data from a `PMTiles` source.
///
/// An `offset` is a position in the archive, which can be larger than the address space
/// of a 32-bit target, so it is a `u64`. A `length` is the size of the bytes returned
/// in memory, so it is a `usize`.
pub trait SyncBackend {
    /// Reads up to `length` bytes starting at `offset`.
    ///
    /// The reader only asks for a `length` greater than 0, with `offset + length` within `u64`.
    ///
    /// # Errors
    ///
    /// Returns an error if the read fails.
    fn read(&self, offset: u64, length: usize) -> PmtResult<BackendResponse>;
}

/// Reads an archive held in memory.
impl SyncBackend for Bytes {
    fn read(&self, offset: u64, length: usize) -> PmtResult<BackendResponse> {
        read_bytes(self, offset, length)
    }
}

/// Reads a local file with positioned reads, so that concurrent reads need no locking.
///
/// A read from past the end of the file is empty rather than an error, because the file's length
/// is not known without another system call.
#[cfg(any(unix, windows))]
impl SyncBackend for File {
    fn read(&self, offset: u64, length: usize) -> PmtResult<BackendResponse> {
        let mut buf = vec![0; length];
        let len = read_full_at(self, &mut buf, offset)?;
        buf.truncate(len);
        Ok(BackendResponse::new(Bytes::from(buf)))
    }
}

/// Runs a [`SyncBackend`] as an [`AsyncBackend`]: each read is done before its future is returned.
struct Blocking<B>(B);

impl<B: SyncBackend> AsyncBackend for Blocking<B> {
    fn read(
        &self,
        offset: u64,
        length: usize,
    ) -> impl Future<Output = PmtResult<BackendResponse>> + Send {
        future::ready(self.0.read(offset, length))
    }
}

/// Wakes the thread that is blocked in [`block_on`].
struct ThreadWaker(Thread);

impl Wake for ThreadWaker {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// Runs a future to completion on the current thread, without an async runtime.
///
/// With a [`SyncBackend`], the reader's futures complete on their first poll, unless the cache
/// waits: [`MokaCache`](crate::MokaCache) makes a lookup wait while another thread reads the
/// same directory, and wakes it when that read is done.
fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let waker = Waker::from(Arc::new(ThreadWaker(thread::current())));
    let mut cx = Context::from_waker(&waker);
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(output) => return output,
            Poll::Pending => thread::park(),
        }
    }
}

/// A synchronous reader for `PMTiles` archives, which needs no async runtime.
///
/// It runs an [`AsyncPmTilesReader`] over a [`SyncBackend`], so it reads and validates
/// archives exactly the same way.
///
/// ```no_run
/// use pmtiles::{PmTilesReader, PmtResult, TileCoord};
///
/// fn get_tile(z: u8, x: u32, y: u32) -> PmtResult<Option<bytes::Bytes>> {
///     let reader = PmTilesReader::new_with_path("example.pmtiles")?;
///     reader.get_tile(TileCoord::new(z, x, y)?)
/// }
/// ```
pub struct PmTilesReader<B, C = NoCache> {
    inner: Arc<AsyncPmTilesReader<Blocking<B>, C>>,
}

#[cfg(any(unix, windows))]
impl PmTilesReader<File, NoCache> {
    /// Creates a new reader from a file path.
    ///
    /// On Windows, no other process can open the file for writing while the reader has it open.
    ///
    /// # Errors
    ///
    /// This function will return an error if the
    /// - file cannot be opened,
    /// - header/root directory cannot be read or
    /// - root directory is malformed
    pub fn new_with_path<P: AsRef<Path>>(path: P) -> PmtResult<Self> {
        Self::try_from_source(read_options().open(path)?)
    }
}

impl<B: SyncBackend + Sync + Send> PmTilesReader<B, NoCache> {
    /// Creates a new reader from a specified source and validates the provided `PMTiles` archive is valid.
    ///
    /// # Errors
    ///
    /// This function will return an error if
    /// - the backend fails to read the header/root directory or
    /// - or if the root directory is malformed
    pub fn try_from_source(backend: B) -> PmtResult<Self> {
        Self::try_from_source_and_cache(backend, NoCache)
    }
}

impl<B: SyncBackend + Sync + Send, C: DirectoryCache + Sync + Send> PmTilesReader<B, C> {
    /// Creates a new reader with a directory cache from a specified source, and validates the
    /// provided `PMTiles` archive is valid.
    ///
    /// # Errors
    ///
    /// This function will return an error if
    /// - the backend fails to read the header/root directory,
    /// - or if the header or root directory is malformed
    pub fn try_from_source_and_cache(backend: B, cache: C) -> PmtResult<Self> {
        let reader = block_on(AsyncPmTilesReader::try_from_source_and_cache(
            Blocking(backend),
            cache,
        ))?;
        Ok(Self {
            inner: Arc::new(reader),
        })
    }

    /// Fetches tile data using either [`TileCoord`](crate::TileCoord) or [`TileId`] to locate
    /// the tile. See [`AsyncPmTilesReader::get_tile`].
    ///
    /// # Errors
    ///
    /// This function will return an error if the backend fails to read the directories or
    /// tile data, or if they are malformed.
    pub fn get_tile<Id: Into<TileId>>(&self, tile_id: Id) -> PmtResult<Option<Bytes>> {
        block_on(self.inner.get_tile(tile_id))
    }

    /// Fetches tile bytes from the archive, decompressing them if the tile is compressed.
    /// See [`AsyncPmTilesReader::get_tile_decompressed`].
    ///
    /// # Errors
    ///
    /// This function will return an error if the tile cannot be read or decompressed.
    pub fn get_tile_decompressed<Id: Into<TileId>>(&self, tile_id: Id) -> PmtResult<Option<Bytes>> {
        block_on(self.inner.get_tile_decompressed(tile_id))
    }

    /// Access header information.
    #[must_use]
    pub fn get_header(&self) -> &Header {
        self.inner.get_header()
    }

    /// Gets metadata from the archive. See [`AsyncPmTilesReader::get_metadata`].
    ///
    /// # Errors
    ///
    /// This function will return an error if the metadata is empty, cannot be read or
    /// decompressed, or is not valid UTF-8.
    pub fn get_metadata(&self) -> PmtResult<String> {
        block_on(self.inner.get_metadata())
    }

    /// Returns an iterator over all tile entries in the archive. Directory entries are
    /// traversed, and not included in the result. See [`AsyncPmTilesReader::entries`].
    #[cfg(feature = "iter")]
    pub fn entries<'a>(&self) -> impl Iterator<Item = PmtResult<DirEntry>> + 'a
    where
        B: 'a,
        C: 'a,
    {
        let mut entries = Arc::clone(&self.inner).entries();
        iter::from_fn(move || block_on(entries.next()))
    }

    /// Parses the metadata and combines it with header data to produce a `TileJSON` object.
    /// See [`AsyncPmTilesReader::parse_tilejson`].
    ///
    /// # Errors
    ///
    /// This function will return an error if reading or parsing the metadata fails.
    #[cfg(feature = "tilejson")]
    pub fn parse_tilejson(&self, sources: Vec<String>) -> PmtResult<TileJSON> {
        block_on(self.inner.parse_tilejson(sources))
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::tests::{RASTER_FILE, VECTOR_FILE};
    use crate::{Compression, HashMapCache, PmtError, TileCoord};

    fn memory(path: &str) -> PmTilesReader<Bytes> {
        PmTilesReader::try_from_source(Bytes::from(fs::read(path).unwrap())).unwrap()
    }

    #[test]
    fn reads_tiles_from_memory() {
        let reader = memory(RASTER_FILE);
        let tile = reader.get_tile(TileCoord::new(3, 4, 2).unwrap()).unwrap();
        assert!(tile.is_some_and(|tile| !tile.is_empty()));

        let reader = memory(VECTOR_FILE);
        let leaf_tile = reader.get_tile(TileCoord::new(12, 2174, 1492).unwrap());
        assert!(leaf_tile.unwrap().is_some());
        let missing = reader.get_tile(TileCoord::new(6, 31, 23).unwrap());
        assert!(missing.unwrap().is_none());
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn file_matches_memory() {
        let from_file = PmTilesReader::new_with_path(VECTOR_FILE).unwrap();
        let from_memory = memory(VECTOR_FILE);
        for coord in [(0, 0, 0), (6, 31, 23), (12, 2174, 1492)] {
            let coord = TileCoord::new(coord.0, coord.1, coord.2).unwrap();
            assert_eq!(
                from_file.get_tile(coord).unwrap(),
                from_memory.get_tile(coord).unwrap()
            );
        }
        assert_eq!(
            from_file.get_metadata().unwrap(),
            from_memory.get_metadata().unwrap()
        );
    }

    /// A read is clamped to the end of the file, and is empty at or past it.
    #[cfg(any(unix, windows))]
    #[test]
    fn file_reads_stop_at_the_end() {
        let file = File::open(RASTER_FILE).unwrap();
        let data = fs::read(RASTER_FILE).unwrap();
        let len = data.len() as u64;

        let res = SyncBackend::read(&file, len - 10, 100).unwrap();
        assert_eq!(res.bytes, data[data.len() - 10..]);
        assert!(SyncBackend::read(&file, len, 10).unwrap().bytes.is_empty());
        assert!(
            SyncBackend::read(&file, len + 10, 10)
                .unwrap()
                .bytes
                .is_empty()
        );
    }

    #[test]
    fn decompresses_tiles_and_metadata() {
        let reader = memory(VECTOR_FILE);
        assert_eq!(reader.get_header().tile_compression, Compression::Gzip);
        let coord = TileCoord::new(0, 0, 0).unwrap();
        let tile = reader.get_tile(coord).unwrap().unwrap();
        let decompressed = reader.get_tile_decompressed(coord).unwrap().unwrap();
        assert!(decompressed.len() > tile.len());
        assert!(reader.get_metadata().unwrap().starts_with('{'));
    }

    #[test]
    fn reads_leaf_directories_with_a_cache() {
        let data = Bytes::from(fs::read("fixtures/leaf.pmtiles").unwrap());
        let reader =
            PmTilesReader::try_from_source_and_cache(data, HashMapCache::default()).unwrap();
        // Read twice, so the second lookup uses the cached leaf directory
        for _ in 0..2 {
            reader.get_tile(TileId::new(0).unwrap()).unwrap();
        }
    }

    #[test]
    fn rejects_invalid_archives() {
        assert!(matches!(
            PmTilesReader::try_from_source(Bytes::from_static(b"not an archive")),
            Err(PmtError::InvalidMagicNumber)
        ));
    }

    #[cfg(feature = "iter")]
    #[test]
    fn iterates_over_entries() {
        let reader = memory(VECTOR_FILE);
        let mut count = 0;
        for entry in reader.entries() {
            let entry = entry.unwrap();
            let coord = entry.iter_coords().next().unwrap();
            assert!(reader.get_tile(coord).unwrap().is_some());
            count += 1;
        }
        assert!(count > 0);
    }

    #[cfg(feature = "tilejson")]
    #[test]
    fn parses_tilejson() {
        let tilejson = memory(VECTOR_FILE).parse_tilejson(Vec::new()).unwrap();
        assert!(tilejson.attribution.is_some());
    }

    /// A future that is not ready on its first poll still completes, once it is woken.
    #[cfg(not(target_family = "wasm"))]
    #[test]
    fn block_on_waits_until_woken() {
        let mut polled = false;
        let future = future::poll_fn(|cx| {
            if polled {
                Poll::Ready(42)
            } else {
                polled = true;
                let waker = cx.waker().clone();
                thread::spawn(move || waker.wake());
                Poll::Pending
            }
        });
        assert_eq!(block_on(future), 42);
    }
}
