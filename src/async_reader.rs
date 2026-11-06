#[cfg(feature = "iter")]
use std::collections::{HashSet, VecDeque};
#[cfg(any(unix, windows))]
use std::fs::{File, OpenOptions};
use std::future::Future;
use std::io;
use std::ops::Range;
#[cfg(feature = "iter")]
use std::sync::Arc;
#[cfg(feature = "tokio")]
use std::time::Duration;

#[cfg(feature = "iter")]
use async_stream::try_stream;
use bytes::Bytes;
#[cfg(feature = "iter")]
use futures_util::stream::BoxStream;
#[cfg(feature = "tilejson")]
use tilejson::{TileJSON, VectorLayer};
#[cfg(feature = "tokio")]
use tokio::runtime::Handle;

use crate::header::{HEADER_SIZE, MAX_INITIAL_BYTES};
use crate::{
    Compression, DirEntry, Directory, DirectoryCache, Header, NoCache, PmtError, PmtResult, TileId,
};

/// The deepest leaf directory a tile lookup reads, counting one listed in the root directory as 0.
const MAX_LEAF_DEPTH: u8 = 5;

/// The response from a backend [`AsyncBackend::read`] call.
#[derive(Debug)]
pub struct BackendResponse {
    /// The bytes read from the backend.
    pub bytes: Bytes,
    /// An optional version string for detecting source data changes (e.g. HTTP E-Tags).
    /// A None means the backend does not have a way of discriminating `PMTiles` data versions
    /// or a data version was not able to be acquired for this request.
    pub data_version_string: Option<String>,
}

impl BackendResponse {
    /// Creates a `BackendResponse` with no version string.
    pub fn new(bytes: Bytes) -> Self {
        Self {
            bytes,
            data_version_string: None,
        }
    }

    /// Creates a `BackendResponse` with a version string.
    pub fn new_with_version(bytes: Bytes, data_version_string: String) -> Self {
        Self {
            bytes,
            data_version_string: Some(data_version_string),
        }
    }
}

/// Reads up to `length` bytes of an archive held in `bytes`, starting at `offset`.
///
/// An offset at exactly the end is a valid empty read; past it is an error. Short reads are the
/// normal path: the reader always asks for `MAX_INITIAL_BYTES` up front, which is more than the
/// total size of a small archive.
pub(crate) fn read_bytes(bytes: &Bytes, offset: u64, length: usize) -> PmtResult<BackendResponse> {
    let start = usize::try_from(offset)
        .ok()
        .filter(|start| *start <= bytes.len())
        .ok_or_else(|| PmtError::Reading(io::ErrorKind::UnexpectedEof.into()))?;
    let end = start.saturating_add(length).min(bytes.len());
    Ok(BackendResponse::new(bytes.slice(start..end)))
}

/// Options to open a file for reading. Unix cannot prevent writes to it: its file locks are
/// advisory, and tools that overwrite files, such as `cp`, do not take them.
#[cfg(unix)]
pub(crate) fn read_options() -> OpenOptions {
    let mut options = File::options();
    options.read(true);
    options
}

/// Options to open a file for reading, which keep other processes from opening it for writing
/// while it is open, so it cannot change in place. It can still be renamed over or deleted,
/// which is how an archive should be updated.
#[cfg(windows)]
pub(crate) fn read_options() -> OpenOptions {
    use std::os::windows::fs::OpenOptionsExt as _;
    let mut options = File::options();
    // `FILE_SHARE_READ | FILE_SHARE_DELETE`, without `FILE_SHARE_WRITE`
    options.read(true).share_mode(0x1 | 0x4);
    options
}

/// Reads from `file` at `offset` until `buf` is full or the file ends, returning the number of
/// bytes read. Positioned reads leave the file's cursor alone, so threads can share the file.
#[cfg(any(unix, windows))]
pub(crate) fn read_full_at(file: &File, buf: &mut [u8], offset: u64) -> io::Result<usize> {
    #[cfg(unix)]
    use std::os::unix::fs::FileExt as _;
    #[cfg(windows)]
    use std::os::windows::fs::FileExt as _;

    let mut filled = 0;
    while filled < buf.len() {
        let (_, rest) = buf.split_at_mut(filled);
        let offset = offset + filled as u64;
        #[cfg(unix)]
        let result = file.read_at(rest, offset);
        #[cfg(windows)]
        let result = file.seek_read(rest, offset);
        match result {
            // The end of the file
            Ok(0) => break,
            Ok(len) => filled += len,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => return Err(err),
        }
    }
    Ok(filled)
}

/// Decoding estimated to take longer than this runs on Tokio's blocking thread pool by default,
/// the upper end of Tokio's guideline for how long a task may run between `.await`s.
#[cfg(feature = "tokio")]
const DEFAULT_BLOCKING_DECODE: Duration = Duration::from_micros(100);

/// An asynchronous reader for `PMTiles` archives.
pub struct AsyncPmTilesReader<B, C = NoCache> {
    backend: B,
    cache: C,
    header: Header,
    root_directory: Directory,
    initial_data_version_string: Option<String>,
    /// Decoding estimated to take longer than this runs on Tokio's blocking thread pool.
    #[cfg(feature = "tokio")]
    blocking_decode: Option<Duration>,
}

impl<B: AsyncBackend + Sync + Send> AsyncPmTilesReader<B, NoCache> {
    /// Creates a new reader from a specified source and validates the provided `PMTiles` archive is valid.
    ///
    /// Note: Prefer using `new_with_*` methods.
    ///
    /// # Errors
    ///
    /// This function will return an error if
    /// - the backend fails to read the header/root directory or
    /// - or if the root directory is malformed
    pub async fn try_from_source(backend: B) -> PmtResult<Self> {
        Self::try_from_source_and_cache(backend, NoCache).await
    }
}

impl<B: AsyncBackend + Sync + Send, C: DirectoryCache + Sync + Send> AsyncPmTilesReader<B, C> {
    /// Creates a new reader from a specified source with a directory cache, and validates the provided `PMTiles` archive is valid.
    ///
    /// Note: Prefer using `new_with_*` methods.
    ///
    /// # Errors
    ///
    /// This function will return an error if
    /// - the backend fails to read the header/root directory,
    /// - or if the header or root directory is malformed
    #[deprecated(since = "0.27.0", note = "renamed to `try_from_source_and_cache`")]
    pub async fn try_from_cached_source(backend: B, cache: C) -> PmtResult<Self> {
        Self::try_from_source_and_cache(backend, cache).await
    }

    /// Creates a new reader from a specified source with a directory cache, and validates the provided `PMTiles` archive is valid.
    ///
    /// The cache keeps directories between tile lookups, so that repeated lookups skip reading them.
    ///
    /// Note: Prefer using `new_with_*` methods.
    ///
    /// # Errors
    ///
    /// This function will return an error if
    /// - the backend fails to read the header/root directory,
    /// - or if the header or root directory is malformed
    pub async fn try_from_source_and_cache(backend: B, cache: C) -> PmtResult<Self> {
        // Read the first 127 and up to 16,384 bytes to ensure we can initialize the header and root directory.
        let initial_response = backend.read(0, MAX_INITIAL_BYTES).await?;
        let mut initial_bytes = initial_response.bytes;
        // Every later read is `read_exact`, which returns the same error for any other length
        if initial_bytes.len() > MAX_INITIAL_BYTES {
            return Err(PmtError::UnexpectedNumberOfBytesReturned(
                MAX_INITIAL_BYTES,
                initial_bytes.len(),
            ));
        }
        let initial_data_version_string = initial_response.data_version_string;

        // Fewer bytes than a header are still parsed, to report the version they are for
        let header_len = initial_bytes.len().min(HEADER_SIZE);
        let header = Header::try_from_bytes(initial_bytes.split_to(header_len))?;
        if !sections_fit_in_u64(&header) {
            return Err(PmtError::InvalidHeader);
        }

        let Some(root_range) = root_directory_range(&header, initial_bytes.len()) else {
            return Err(PmtError::InvalidHeader);
        };
        let directory_bytes = initial_bytes.slice(root_range);

        // The root directory is at most 16 KiB, and is decoded only once
        let root_directory =
            read_compressed_directory(header.internal_compression, directory_bytes)?;

        Ok(Self {
            backend,
            cache,
            header,
            root_directory,
            initial_data_version_string,
            #[cfg(feature = "tokio")]
            blocking_decode: Some(DEFAULT_BLOCKING_DECODE),
        })
    }

    /// Fetches tile data using either [`TileCoord`](crate::TileCoord) or [`TileId`] to locate the tile.
    ///
    /// ```no_run
    /// # async fn test() {
    /// #     let backend = pmtiles::FileBackend::try_from("").await.unwrap();
    /// #     let reader = pmtiles::AsyncPmTilesReader::try_from_source(backend).await.unwrap();
    /// // Using a tile (z, x, y) coordinate to fetch a tile
    /// let coord = pmtiles::TileCoord::new(0, 0, 0).unwrap();
    /// let tile = reader.get_tile(coord).await.unwrap();
    /// // Using a tile ID to fetch a tile
    /// let tile_id = pmtiles::TileId::from(coord);
    /// let tile = reader.get_tile(tile_id).await.unwrap();
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// This function will return an error if the
    /// - header/root directory cannot be read or
    /// - backend fails to read tile data
    /// - underlying `PMTiles` archive has changed since initialization
    pub async fn get_tile<Id: Into<TileId>>(&self, tile_id: Id) -> PmtResult<Option<Bytes>> {
        let Some(entry) = self.find_tile_entry(tile_id.into()).await? else {
            return Ok(None);
        };

        let offset = entry_offset(self.header.data_offset, self.header.data_length, &entry)?;
        Ok(Some(
            self.read_exact_unmodified(offset, entry.length as usize)
                .await?,
        ))
    }

    /// Fetches tile bytes from the archive.
    /// If the tile is compressed, it will be decompressed.
    ///
    /// # Errors
    ///
    /// This function will return an error if the
    /// - header/root directory cannot be read,
    /// - backend fails to read tile data or
    /// - tile header indicates that the tile is compressed and it cannot be decompressed
    pub async fn get_tile_decompressed<Id: Into<TileId>>(
        &self,
        tile_id: Id,
    ) -> PmtResult<Option<Bytes>> {
        Ok(if let Some(data) = self.get_tile(tile_id).await? {
            Some(
                self.decode(self.header.tile_compression, data, Compression::decompress)
                    .await?,
            )
        } else {
            None
        })
    }

    /// Sets how long decoding may be expected to take on the calling thread, which is 100 µs by
    /// default. Decoding is decompressing directories, metadata and tiles, and parsing
    /// directories. Decoding estimated to take longer runs on Tokio's blocking thread pool, so
    /// that it does not hold up other tasks on the same thread: a large leaf directory can take
    /// milliseconds. `None` decodes everything on the calling thread, as does running outside
    /// a Tokio runtime.
    ///
    /// The estimate comes from the compressed size and the compression, at rates measured on a
    /// recent desktop CPU. Handing work to the blocking pool costs about 15-40 µs.
    #[cfg(feature = "tokio")]
    #[must_use]
    pub fn blocking_decode_above(mut self, threshold: Option<Duration>) -> Self {
        self.blocking_decode = threshold;
        self
    }

    /// Access header information.
    pub fn get_header(&self) -> &Header {
        &self.header
    }

    /// Gets metadata from the archive.
    ///
    /// Note: by spec, this should be valid JSON. This method currently returns a [String].
    /// This may change in the future.
    ///
    /// # Errors
    ///
    /// This function will return an error if the
    /// - metadata is empty, so it is not a JSON object,
    /// - backend fails to read the metadata or
    /// - metadata cannot be decompressed or
    /// - is not valid UTF-8 or
    /// - underlying `PMTiles` archive has changed since initialization
    pub async fn get_metadata(&self) -> PmtResult<String> {
        if self.header.metadata_length == 0 {
            return Err(PmtError::InvalidMetadata);
        }
        let offset = self.header.metadata_offset;
        let length =
            usize::try_from(self.header.metadata_length).map_err(|_| PmtError::InvalidHeader)?;
        let metadata = self.read_exact_unmodified(offset, length).await?;

        let decompressed_metadata = self
            .decode(
                self.header.internal_compression,
                metadata,
                Compression::decompress,
            )
            .await?;

        Ok(String::from_utf8(Vec::from(decompressed_metadata))?)
    }

    /// Return an async stream over all tile entries in the archive. Directory entries are traversed, and not included in the result.
    /// Because this function requires the reader for the duration of the stream, you need to wrap the reader in an `Arc`.
    ///
    /// ```
    /// # use std::sync::Arc;
    /// # use pmtiles::{AsyncPmTilesReader, FileBackend};
    /// # use futures_util::TryStreamExt as _;
    /// #[tokio::main(flavor="current_thread")]
    /// async fn main() -> Result<(), pmtiles::PmtError> {
    ///     let backend = FileBackend::try_from("fixtures/protomaps(vector)ODbL_firenze.pmtiles").await?;
    ///     let reader = Arc::new(AsyncPmTilesReader::try_from_source(backend).await?);
    ///     let mut entries = reader.entries();
    ///     while let Some(entry) = entries.try_next().await? {
    ///        // ... do something with entry ...
    ///     }
    ///     Ok(())
    /// }
    /// ```
    #[cfg(feature = "iter")]
    pub fn entries<'a>(self: Arc<Self>) -> BoxStream<'a, PmtResult<DirEntry>>
    where
        B: 'a,
        C: 'a,
    {
        Box::pin(try_stream! {
            let mut queue = VecDeque::new();
            // A valid archive lists each leaf directory once, so a repeat is a cycle
            let mut leaf_offsets = HashSet::new();

            queue.extend(self.root_directory.entries.iter().copied());

            while let Some(entry) = queue.pop_front() {
                if entry.is_leaf() {
                    if !leaf_offsets.insert(entry.offset) {
                        Err(PmtError::InvalidEntry)?;
                    }
                    let offset =
                        entry_offset(self.header.leaf_offset, self.header.leaf_length, &entry)?;
                    let leaf_dir = self.read_directory(offset, entry.length as usize).await?;
                    // enqueue all entries in the leaf directory
                    for leaf_entry in leaf_dir.entries {
                        queue.push_back(leaf_entry);
                    }
                } else {
                    yield entry;
                }
            }
        })
    }

    #[cfg(feature = "tilejson")]
    /// Parses the metadata and combines it with header data to produce a `TileJSON` object.
    ///
    /// # Errors
    ///
    /// This function will return an error if
    /// - reading or parsing the metadata fails or
    /// - metadata contains invalid JSON
    pub async fn parse_tilejson(&self, sources: Vec<String>) -> PmtResult<TileJSON> {
        use serde_json::Value;

        let meta = self.get_metadata().await?;
        let meta: Value = serde_json::from_str(&meta).map_err(|_| PmtError::InvalidMetadata)?;
        let Value::Object(meta) = meta else {
            return Err(PmtError::InvalidMetadata);
        };

        let mut tj = self.header.get_tilejson(sources);
        for (key, value) in meta {
            if key == "minzoom" || key == "maxzoom" {
                // We already have the correct values from the header, so just drop these
                // attributes from the metadata silently, don't overwrite known-good
                // values, regardless of their type.
            } else if let Value::String(v) = value {
                if key == "description" {
                    tj.description = Some(v);
                } else if key == "attribution" {
                    tj.attribution = Some(v);
                } else if key == "legend" {
                    tj.legend = Some(v);
                } else if key == "name" {
                    tj.name = Some(v);
                } else if key == "version" {
                    tj.version = Some(v);
                } else {
                    tj.other.insert(key, Value::String(v));
                }
            } else if key == "vector_layers" {
                if let Ok(v) = serde_json::from_value::<Vec<VectorLayer>>(value) {
                    tj.vector_layers = Some(v);
                } else {
                    return Err(PmtError::InvalidMetadata);
                }
            } else {
                tj.other.insert(key, value);
            }
        }
        Ok(tj)
    }

    /// Recursively locates a tile in the archive.
    async fn find_tile_entry(&self, tile_id: TileId) -> PmtResult<Option<DirEntry>> {
        let entry = self.root_directory.find_tile_id(tile_id);
        if let Some(entry) = entry
            && entry.is_leaf()
        {
            return self.find_entry_rec(tile_id, entry, 0).await;
        }

        Ok(entry.copied())
    }

    async fn find_entry_rec(
        &self,
        tile_id: TileId,
        entry: &DirEntry,
        depth: u8,
    ) -> PmtResult<Option<DirEntry>> {
        if depth > MAX_LEAF_DEPTH {
            return Err(PmtError::InvalidEntry);
        }
        // the recursion is done as two functions because it is a bit cleaner,
        // and it allows the directory to be cached later without cloning it first.
        let offset = entry_offset(self.header.leaf_offset, self.header.leaf_length, entry)?;

        let entry = self
            .cache
            .get_dir_entry_or_insert(offset, tile_id, async move {
                // Boxed, so that a cache hit, which never polls it, does not move its state
                Box::pin(self.read_directory(offset, entry.length as usize)).await
            })
            .await?;

        if let Some(ref entry) = entry
            && entry.is_leaf()
        {
            return Box::pin(self.find_entry_rec(tile_id, entry, depth + 1)).await;
        }

        Ok(entry)
    }

    async fn read_directory(&self, offset: u64, length: usize) -> PmtResult<Directory> {
        let data = self.read_exact_unmodified(offset, length).await?;
        self.decode(
            self.header.internal_compression,
            data,
            read_compressed_directory,
        )
        .await
    }

    /// Runs `decode` on Tokio's blocking thread pool if it is estimated to take longer than the
    /// threshold, and there is a Tokio runtime to run it on, or on this thread otherwise.
    #[cfg_attr(
        not(feature = "tokio"),
        expect(
            clippy::unused_async,
            clippy::unused_async_trait_impl,
            reason = "only offloading to Tokio awaits"
        )
    )]
    async fn decode<T: Send + 'static>(
        &self,
        compression: Compression,
        bytes: Bytes,
        decode: fn(Compression, Bytes) -> PmtResult<T>,
    ) -> PmtResult<T> {
        #[cfg(feature = "tokio")]
        if self
            .blocking_decode
            .is_some_and(|threshold| decode_estimate(compression, bytes.len()) > threshold)
            && let Ok(runtime) = Handle::try_current()
        {
            return runtime
                .spawn_blocking(move || decode(compression, bytes))
                .await
                .map_err(io::Error::other)?;
        }
        decode(compression, bytes)
    }

    /// Reads exactly `length` bytes starting at `offset`, failing if the archive changed since initialization.
    ///
    /// The initial data version string (stored at instantiation time) is compared against the one of this read,
    /// but only if both are set.
    /// If an initial data version string was not available, the backend does not support exposing version strings.
    /// If a current data version string is not available, this request was unable to extract a data version string,
    /// and the check is skipped.
    async fn read_exact_unmodified(&self, offset: u64, length: usize) -> PmtResult<Bytes> {
        let response = self.backend.read_exact(offset, length).await?;
        if let (Some(expected), Some(actual)) = (
            &self.initial_data_version_string,
            &response.data_version_string,
        ) && expected != actual
        {
            return Err(PmtError::SourceModified);
        }
        Ok(response.bytes)
    }
}

/// Estimates how long decoding `len` compressed bytes takes, at rates per compressed byte
/// measured on a recent desktop CPU, near the slower end so as not to underestimate. Parsing a
/// directory costs little next to decompressing it, so the same rates cover directories.
#[cfg(feature = "tokio")]
fn decode_estimate(compression: Compression, len: usize) -> Duration {
    let ns_per_byte: u64 = match compression {
        Compression::Gzip => 20,
        Compression::Brotli => 30,
        Compression::Zstd => 6,
        Compression::None | Compression::Unknown => 0,
    };
    let len = u64::try_from(len).unwrap_or(u64::MAX);
    Duration::from_nanos(len.saturating_mul(ns_per_byte))
}

fn read_compressed_directory(compression: Compression, bytes: Bytes) -> PmtResult<Directory> {
    Directory::try_from(compression.decompress(bytes)?)
}

/// Returns where the root directory is within the `available` bytes read after the header,
/// or `None` if the header places it anywhere else.
fn root_directory_range(header: &Header, available: usize) -> Option<Range<usize>> {
    // The header's offsets count from the start of the archive, which includes the header.
    let start = usize::try_from(header.root_offset)
        .ok()?
        .checked_sub(HEADER_SIZE)?;
    let end = start.checked_add(usize::try_from(header.root_length).ok()?)?;
    (end <= available).then_some(start..end)
}

/// Returns whether the end of each section fits in `u64`, so that no read within a section
/// overflows when computing its end.
fn sections_fit_in_u64(header: &Header) -> bool {
    [
        (header.metadata_offset, header.metadata_length),
        (header.leaf_offset, header.leaf_length),
        (header.data_offset, header.data_length),
    ]
    .into_iter()
    .all(|(offset, length)| offset.checked_add(length).is_some())
}

/// Returns the offset of an entry in the archive, failing if a malformed directory points it
/// outside of its section, so that it is never read.
fn entry_offset(section_offset: u64, section_length: u64, entry: &DirEntry) -> PmtResult<u64> {
    entry
        .offset
        .checked_add(u64::from(entry.length))
        .filter(|end| *end <= section_length)
        .and_then(|_| section_offset.checked_add(entry.offset))
        .ok_or(PmtError::InvalidEntry)
}

/// A trait for asynchronous backends that can read data from a `PMTiles` source.
///
/// An `offset` is a position in the archive, which can be larger than the address space
/// of a 32-bit target, so it is a `u64`. A `length` is the size of the bytes returned
/// in memory, so it is a `usize`.
pub trait AsyncBackend {
    /// Reads exactly `length` bytes starting at `offset`
    fn read_exact(
        &self,
        offset: u64,
        length: usize,
    ) -> impl Future<Output = PmtResult<BackendResponse>> + Send
    where
        Self: Sync,
    {
        async move {
            let response = self.read(offset, length).await?;

            if response.bytes.len() != length {
                return Err(PmtError::UnexpectedNumberOfBytesReturned(
                    length,
                    response.bytes.len(),
                ));
            }

            Ok(response)
        }
    }

    /// Reads up to `length` bytes starting at `offset`.
    ///
    /// The reader only asks for a `length` greater than 0, with `offset + length` within `u64`.
    fn read(
        &self,
        offset: u64,
        length: usize,
    ) -> impl Future<Output = PmtResult<BackendResponse>> + Send;
}

#[cfg(test)]
#[cfg(all(feature = "file-async-tokio", any(unix, windows)))]
mod tests {
    use std::pin::pin;
    #[cfg(feature = "iter")]
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::task::{Context, Poll, Waker};
    use std::time::Duration;
    use std::{fs, future};

    use bytes::Bytes;
    use rstest::rstest;
    #[cfg(feature = "tilejson")]
    use tilejson::TileJSON;

    use super::decode_estimate;
    use crate::tests::{RASTER_FILE, VECTOR_FILE};
    use crate::{
        AsyncBackend, AsyncPmTilesReader, BackendResponse, Compression, FileBackend, PmtError,
        PmtResult, TileCoord,
    };
    #[cfg(feature = "iter")]
    use crate::{DirEntry, HashMapCache, TileId};

    fn id(z: u8, x: u32, y: u32) -> TileCoord {
        TileCoord::new(z, x, y).unwrap()
    }

    #[rstest]
    #[case(RASTER_FILE)]
    #[case(VECTOR_FILE)]
    #[tokio::test]
    async fn open_sanity_check(#[case] file: &str) {
        let backend = FileBackend::try_from(file).await.unwrap();
        AsyncPmTilesReader::try_from_source(backend).await.unwrap();
    }

    /// Byte positions of the header's fields.
    const ROOT_OFFSET: usize = 8;
    const ROOT_LENGTH: usize = 16;
    const METADATA_OFFSET: usize = 24;
    const METADATA_LENGTH: usize = 32;
    const LEAF_OFFSET: usize = 40;
    const LEAF_LENGTH: usize = 48;
    const DATA_OFFSET: usize = 56;
    const DATA_LENGTH: usize = 64;
    const INTERNAL_COMPRESSION: usize = 97;

    /// A root directory outside the initial bytes is an invalid header, not a panic or a misread.
    #[rstest]
    #[case::before_header(ROOT_OFFSET, 0)]
    #[case::past_initial_bytes(ROOT_OFFSET, 16_384)]
    #[case::above_4_gib(ROOT_OFFSET, (1 << 32) + 127)] // truncates to a valid offset on 32-bit
    #[case::too_long(ROOT_LENGTH, u64::MAX)]
    #[tokio::test]
    async fn rejects_root_directory_outside_initial_bytes(
        #[case] field: usize,
        #[case] value: u64,
    ) {
        let mut data = fs::read(RASTER_FILE).unwrap();
        data[field..field + 8].copy_from_slice(&value.to_le_bytes());

        let result = AsyncPmTilesReader::try_from_source(VersionedBackend::new(&data, None)).await;
        assert!(matches!(result, Err(PmtError::InvalidHeader)));
    }

    /// Data shorter than a header is an invalid header.
    #[tokio::test]
    async fn rejects_truncated_header() {
        let data = fs::read(RASTER_FILE).unwrap();
        let backend = VersionedBackend::new(&data[..100], None);

        let result = AsyncPmTilesReader::try_from_source(backend).await;
        assert!(matches!(result, Err(PmtError::InvalidHeader)));
    }

    /// A short v2 archive is reported as an unsupported version, not as an invalid header.
    #[tokio::test]
    async fn rejects_short_v2_archive() {
        let backend = VersionedBackend::new(b"PM\x02\x00", None);

        let result = AsyncPmTilesReader::try_from_source(backend).await;
        assert!(matches!(result, Err(PmtError::UnsupportedPmTilesVersion)));
    }

    /// Directories with an unknown internal compression cannot be read.
    #[tokio::test]
    async fn rejects_unsupported_internal_compression() {
        let mut data = include_bytes!("../fixtures/leaf.pmtiles").to_vec();
        data[INTERNAL_COMPRESSION] = 0; // Compression::Unknown

        let result = AsyncPmTilesReader::try_from_source(VersionedBackend::new(&data, None)).await;
        assert!(matches!(
            result,
            Err(PmtError::UnsupportedCompression(Compression::Unknown))
        ));
    }

    /// A section whose end overflows `u64` is an invalid header, not an overflow in a later read.
    #[rstest]
    #[case::metadata_offset(METADATA_OFFSET)]
    #[case::metadata_length(METADATA_LENGTH)]
    #[case::leaf_offset(LEAF_OFFSET)]
    #[case::leaf_length(LEAF_LENGTH)]
    #[case::data_offset(DATA_OFFSET)]
    #[case::data_length(DATA_LENGTH)]
    #[tokio::test]
    async fn rejects_section_end_overflowing_u64(#[case] field: usize) {
        let mut data = include_bytes!("../fixtures/leaf.pmtiles").to_vec();
        data[field..field + 8].copy_from_slice(&u64::MAX.to_le_bytes());

        let result = AsyncPmTilesReader::try_from_source(VersionedBackend::new(&data, None)).await;
        assert!(matches!(result, Err(PmtError::InvalidHeader)));
    }

    /// Empty metadata is not a JSON object, so it is rejected without being read.
    #[tokio::test]
    async fn rejects_empty_metadata() {
        let mut data = include_bytes!("../fixtures/leaf.pmtiles").to_vec();
        data[METADATA_LENGTH..METADATA_LENGTH + 8].copy_from_slice(&0_u64.to_le_bytes());

        let reader = AsyncPmTilesReader::try_from_source(VersionedBackend::new(&data, None))
            .await
            .unwrap();
        let result = reader.get_metadata().await;
        assert!(
            matches!(result, Err(PmtError::InvalidMetadata)),
            "{result:?}"
        );
    }

    #[rstest]
    #[case(id(0, 0, 0), include_bytes!("../fixtures/0_0_0.png"))]
    #[case(id(2, 2, 2), include_bytes!("../fixtures/2_2_2.png"))]
    #[case(id(3, 4, 5), include_bytes!("../fixtures/3_4_5.png"))]
    #[tokio::test]
    async fn get_tiles(#[case] coord: TileCoord, #[case] fixture_bytes: &[u8]) {
        let backend = FileBackend::try_from(RASTER_FILE).await.unwrap();
        let tiles = AsyncPmTilesReader::try_from_source(backend).await.unwrap();
        let tile = tiles.get_tile_decompressed(coord).await.unwrap().unwrap();

        assert_eq!(
            tile.len(),
            fixture_bytes.len(),
            "Expected tile length to match."
        );
        assert_eq!(tile, fixture_bytes, "Expected tile to match fixture.");
    }

    #[rstest]
    #[case(id(0, 0, 0), include_bytes!("../fixtures/0_0_0.png"))]
    #[case(id(2, 2, 2), include_bytes!("../fixtures/2_2_2.png"))]
    #[case(id(3, 4, 5), include_bytes!("../fixtures/3_4_5.png"))]
    #[tokio::test]
    #[cfg(feature = "object-store")]
    async fn get_tiles_object_store(#[case] coord: TileCoord, #[case] fixture_bytes: &[u8]) {
        use object_store::memory::InMemory;
        use object_store::path::Path;
        use object_store::{ObjectStoreExt as _, WriteMultipart};

        use crate::ObjectStoreBackend;

        let store = Box::new(InMemory::new());

        let upload_path = Path::from("file.pmtiles");
        let upload = store.put_multipart(&upload_path).await.unwrap();
        let mut write = WriteMultipart::new(upload);
        write.write(include_bytes!(
            "../fixtures/stamen_toner(raster)CC-BY+ODbL_z3.pmtiles"
        ));
        write.finish().await.unwrap();
        let backend = ObjectStoreBackend::new(store, upload_path);
        let tiles = AsyncPmTilesReader::try_from_source(backend).await.unwrap();
        let tile = tiles.get_tile_decompressed(coord).await.unwrap().unwrap();

        assert_eq!(
            tile.len(),
            fixture_bytes.len(),
            "Expected tile length to match."
        );
        assert_eq!(tile, fixture_bytes, "Expected tile to match fixture.");
    }

    #[tokio::test]
    #[cfg(feature = "object-store")]
    async fn data_version_source_modified() {
        // Tests that if the underlying storage object changes after initial read of a
        // pmtiles header, a subsequent read will fail.
        use std::sync::Arc;

        use object_store::memory::InMemory;
        use object_store::path::Path;
        use object_store::{ObjectStore as _, PutOptions, PutPayload};

        // The test uses the ObjectStoreBackend with an InMemory underlying store since that
        // makes it easy to modify the ETag of a stored object by putting a new "version".
        use crate::ObjectStoreBackend;

        // The exact file here does not matter - it is only important that we fetch a valid
        // tile below.
        let data: &[u8] = include_bytes!("../fixtures/stamen_toner(raster)CC-BY+ODbL_z3.pmtiles");
        let store = Arc::new(InMemory::new());
        let path = Path::from("file.pmtiles");

        let payload: PutPayload = Bytes::copy_from_slice(data).into();
        store
            .put_opts(&path, payload, PutOptions::default())
            .await
            .unwrap();

        // Create the AsyncReader and fetch a tile. The first tile fetch will succeed.
        let backend = ObjectStoreBackend::new(Box::new(store.as_ref().clone()), path.clone());
        let tiles = AsyncPmTilesReader::try_from_source(backend).await.unwrap();
        let result = tiles.get_tile(id(0, 0, 0)).await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_some());

        // Overwrite the object — InMemory underlying storage will increment the ETag.
        let payload: PutPayload = Bytes::copy_from_slice(data).into();
        store
            .put_opts(&path, payload, PutOptions::default())
            .await
            .unwrap();

        let result = tiles.get_tile(id(0, 0, 0)).await;
        assert!(matches!(result, Err(PmtError::SourceModified)));
    }

    struct VersionedBackend(Mutex<(Bytes, Option<String>)>);

    impl VersionedBackend {
        fn new(data: &[u8], version: Option<&str>) -> Self {
            Self(Mutex::new((
                Bytes::copy_from_slice(data),
                version.map(str::to_owned),
            )))
        }

        fn replace(&self, data: &[u8], version: Option<&str>) {
            *self.0.lock().unwrap() = (Bytes::copy_from_slice(data), version.map(str::to_owned));
        }
    }

    impl AsyncBackend for VersionedBackend {
        fn read(
            &self,
            offset: u64,
            length: usize,
        ) -> impl Future<Output = PmtResult<BackendResponse>> + Send {
            let (bytes, version) = self.0.lock().unwrap().clone();
            let offset = usize::try_from(offset).unwrap_or(usize::MAX);
            let start = offset.min(bytes.len());
            let end = offset.saturating_add(length).min(bytes.len());
            let response = BackendResponse {
                bytes: bytes.slice(start..end),
                data_version_string: version,
            };
            future::ready(Ok(response))
        }
    }

    #[tokio::test]
    async fn data_version_source_modified_leaf_directory() {
        let data: &[u8] = include_bytes!("../fixtures/leaf.pmtiles");
        let tiles = AsyncPmTilesReader::try_from_source(VersionedBackend::new(data, Some("v1")))
            .await
            .unwrap();
        let header = tiles.get_header();
        let leaf_start = usize::try_from(header.leaf_offset).unwrap();
        let leaf_end = leaf_start + usize::try_from(header.leaf_length).unwrap();
        let mut replaced = data.to_vec();
        replaced[leaf_start..leaf_end].fill(0);
        tiles.backend.replace(&replaced, Some("v2"));

        let result = tiles.get_tile(id(1, 1, 0)).await;
        assert!(
            matches!(result, Err(PmtError::SourceModified)),
            "expected SourceModified, got {result:?}"
        );

        let result = tiles.get_metadata().await;
        assert!(
            matches!(result, Err(PmtError::SourceModified)),
            "expected SourceModified, got {result:?}"
        );
    }

    #[tokio::test]
    async fn data_version_missing_on_later_read() {
        let data: &[u8] = include_bytes!("../fixtures/leaf.pmtiles");
        let tiles = AsyncPmTilesReader::try_from_source(VersionedBackend::new(data, Some("v1")))
            .await
            .unwrap();
        tiles.backend.replace(data, None);

        let tile = tiles.get_tile(id(1, 1, 0)).await.unwrap().unwrap();
        assert_eq!(tile.as_ref(), b"4");
        tiles.get_metadata().await.unwrap();
    }

    #[tokio::test]
    async fn read_error_after_initialization() {
        let data: &[u8] = include_bytes!("../fixtures/leaf.pmtiles");
        let tiles = AsyncPmTilesReader::try_from_source(VersionedBackend::new(data, Some("v1")))
            .await
            .unwrap();
        let leaf_offset = usize::try_from(tiles.get_header().leaf_offset).unwrap();
        tiles.backend.replace(&data[..leaf_offset], Some("v1"));

        let result = tiles.get_tile(id(1, 1, 0)).await;
        assert!(
            matches!(result, Err(PmtError::UnexpectedNumberOfBytesReturned(_, 0))),
            "expected UnexpectedNumberOfBytesReturned, got {result:?}"
        );
    }

    #[rstest]
    #[case(id(6, 31, 23), false)] // missing tile
    #[case(id(12, 2174, 1492), true)] // existing leaf tile
    #[tokio::test]
    async fn test_tile_existence(#[case] coord: TileCoord, #[case] should_exist: bool) {
        let backend = FileBackend::try_from(VECTOR_FILE).await.unwrap();
        let tiles = AsyncPmTilesReader::try_from_source(backend).await.unwrap();

        let tile = tiles.get_tile(coord).await;
        assert!(tile.is_ok());

        if should_exist {
            assert!(tile.unwrap().is_some());
        } else {
            assert!(tile.unwrap().is_none());
        }

        let tile = tiles.get_tile_decompressed(coord).await.unwrap();
        assert_eq!(tile.is_some(), should_exist);
    }

    #[tokio::test]
    async fn leaf_tile_compressed() {
        let backend = FileBackend::try_from(VECTOR_FILE).await.unwrap();
        let tiles = AsyncPmTilesReader::try_from_source(backend).await.unwrap();
        let coord = id(12, 2174, 1492);

        let tile = tiles.get_tile(coord).await;
        assert!(tile.as_ref().is_ok_and(Option::is_some));
        let tile = tile.unwrap().unwrap();

        let tile_dec = tiles.get_tile_decompressed(coord).await;
        assert!(tile_dec.as_ref().is_ok_and(Option::is_some));
        let tile_dec = tile_dec.unwrap().unwrap();

        assert!(
            tile_dec.len() > tile.len(),
            "Decompressed tile should be larger than compressed tile"
        );
    }

    #[tokio::test]
    async fn get_metadata() {
        let backend = FileBackend::try_from(VECTOR_FILE).await.unwrap();
        let tiles = AsyncPmTilesReader::try_from_source(backend).await.unwrap();

        let metadata = tiles.get_metadata().await.unwrap();
        assert_ne!(metadata, "");
    }

    #[test]
    fn decode_estimate_grows_with_size_and_codec_cost() {
        let gzip = decode_estimate(Compression::Gzip, 5000);
        assert_eq!(gzip, Duration::from_micros(100));
        assert!(decode_estimate(Compression::Brotli, 5000) > gzip);
        assert!(decode_estimate(Compression::Zstd, 5000) < gzip);
        assert!(decode_estimate(Compression::Gzip, 50_000) > gzip);
        assert_eq!(decode_estimate(Compression::None, 5000), Duration::ZERO);
    }

    async fn open_vector(threshold: Option<Duration>) -> AsyncPmTilesReader<FileBackend> {
        let backend = FileBackend::try_from(VECTOR_FILE).await.unwrap();
        let reader = AsyncPmTilesReader::try_from_source(backend).await.unwrap();
        reader.blocking_decode_above(threshold)
    }

    /// Decoding everything on the blocking pool returns the same data as decoding inline.
    #[tokio::test]
    async fn blocking_decode_returns_the_same_data() {
        let inline = open_vector(None).await;
        let blocking = open_vector(Some(Duration::ZERO)).await;
        assert_eq!(
            inline.get_metadata().await.unwrap(),
            blocking.get_metadata().await.unwrap()
        );
        // The second tile is in a leaf directory
        for coord in [id(0, 0, 0), id(12, 2174, 1492)] {
            assert_eq!(
                inline.get_tile_decompressed(coord).await.unwrap(),
                blocking.get_tile_decompressed(coord).await.unwrap()
            );
        }
    }

    /// Outside a Tokio runtime, decoding that would go to the blocking pool runs inline.
    #[test]
    fn blocking_decode_without_a_runtime_runs_inline() {
        let backend = VersionedBackend::new(&fs::read(VECTOR_FILE).unwrap(), None);
        let reader = ready(AsyncPmTilesReader::try_from_source(backend)).unwrap();
        let reader = reader.blocking_decode_above(Some(Duration::ZERO));

        let tile = ready(reader.get_tile_decompressed(id(12, 2174, 1492))).unwrap();
        assert!(tile.is_some());
    }

    /// Polls a future once, which reads from memory always complete.
    fn ready<F: Future>(future: F) -> F::Output {
        let Poll::Ready(output) = pin!(future).poll(&mut Context::from_waker(Waker::noop())) else {
            panic!("reads from memory are always ready");
        };
        output
    }

    /// Returns a reader of an in-memory archive with one tile and the given metadata.
    #[cfg(all(feature = "tilejson", feature = "write"))]
    async fn reader_with_metadata(metadata: &str) -> AsyncPmTilesReader<VersionedBackend> {
        use std::io::Cursor;

        use crate::{PmTilesWriter, TileType};

        let mut buffer = Cursor::new(Vec::new());
        let mut writer = PmTilesWriter::new(TileType::Mvt)
            .min_zoom(2)
            .max_zoom(9)
            .metadata(metadata)
            .create(&mut buffer)
            .unwrap();
        writer.add_tile(id(2, 1, 1), b"tile").unwrap();
        writer.finalize().unwrap();
        let backend = VersionedBackend::new(buffer.get_ref(), None);
        AsyncPmTilesReader::try_from_source(backend).await.unwrap()
    }

    #[tokio::test]
    #[cfg(all(feature = "tilejson", feature = "write"))]
    async fn parse_tilejson_merges_metadata() {
        let reader = reader_with_metadata(
            r#"{
                "name": "name", "description": "description", "attribution": "attribution",
                "legend": "legend", "version": "1.2.3", "minzoom": "5", "maxzoom": "6",
                "custom": "custom", "count": 42,
                "vector_layers": [{"id": "roads", "fields": {"kind": "String"}}]
            }"#,
        )
        .await;
        let tj = reader.parse_tilejson(vec!["url".into()]).await.unwrap();

        assert_eq!(tj.tiles, vec!["url".to_owned()]);
        assert_eq!(tj.name.as_deref(), Some("name"));
        assert_eq!(tj.description.as_deref(), Some("description"));
        assert_eq!(tj.attribution.as_deref(), Some("attribution"));
        assert_eq!(tj.legend.as_deref(), Some("legend"));
        assert_eq!(tj.version.as_deref(), Some("1.2.3"));
        // The zoom range comes from the header, not from the metadata.
        assert_eq!((tj.minzoom, tj.maxzoom), (Some(2), Some(9)));
        assert_eq!(tj.other["custom"], "custom");
        assert_eq!(tj.other["count"], 42);
        assert_eq!(tj.other.len(), 2);
        assert_eq!(tj.vector_layers.unwrap()[0].id, "roads");
    }

    #[rstest]
    #[case::not_json("not json")]
    #[case::not_an_object("[1, 2]")]
    #[case::invalid_vector_layers(r#"{"vector_layers": [{"fields": {}}]}"#)]
    #[tokio::test]
    #[cfg(all(feature = "tilejson", feature = "write"))]
    async fn parse_tilejson_rejects_invalid_metadata(#[case] metadata: &str) {
        let reader = reader_with_metadata(metadata).await;
        let result = reader.parse_tilejson(Vec::new()).await;
        assert!(matches!(result, Err(PmtError::InvalidMetadata)));
    }

    #[rstest]
    #[case(VECTOR_FILE, |tj: &TileJSON| assert!(tj.attribution.is_some()))]
    #[case(RASTER_FILE, |tj: &TileJSON| assert!(tj.other.is_empty()))]
    #[tokio::test]
    #[cfg(feature = "tilejson")]
    async fn test_parse_tilejson(#[case] file: &str, #[case] assertion: impl Fn(&TileJSON)) {
        let backend = FileBackend::try_from(file).await.unwrap();
        let tiles = AsyncPmTilesReader::try_from_source(backend).await.unwrap();
        let tj = tiles.parse_tilejson(Vec::new()).await.unwrap();
        assertion(&tj);
    }

    #[rstest]
    #[case::strings(r#"{"minzoom": "5", "maxzoom": "6"}"#)]
    #[case::numbers(r#"{"minzoom": 5, "maxzoom": 6}"#)]
    #[tokio::test]
    #[cfg(all(feature = "tilejson", feature = "write"))]
    async fn parse_tilejson_ignores_metadata_zoom_range(#[case] metadata: &str) {
        use std::io::Cursor;

        use crate::{PmTilesWriter, TileType};

        let mut buffer = Cursor::new(Vec::new());
        let mut writer = PmTilesWriter::new(TileType::Mvt)
            .min_zoom(2)
            .max_zoom(9)
            .metadata(metadata)
            .create(&mut buffer)
            .unwrap();
        writer.add_tile(id(2, 1, 1), b"tile").unwrap();
        writer.finalize().unwrap();
        let backend = VersionedBackend::new(buffer.get_ref(), None);
        let reader = AsyncPmTilesReader::try_from_source(backend).await.unwrap();

        let tj = reader.parse_tilejson(Vec::new()).await.unwrap();
        assert_eq!((tj.minzoom, tj.maxzoom), (Some(2), Some(9)));
        assert!(tj.other.is_empty(), "{:?}", tj.other);
    }

    #[rstest]
    #[case(id(0, 0, 0), b"0")]
    #[case(id(1, 0, 0), b"1")]
    #[case(id(1, 0, 1), b"2")]
    #[case(id(1, 1, 1), b"3")]
    #[case(id(1, 1, 0), b"4")]
    #[tokio::test]
    async fn test_martin_675(#[case] coord: TileCoord, #[case] expected_contents: &[u8]) {
        let backend = FileBackend::try_from("fixtures/leaf.pmtiles")
            .await
            .unwrap();
        let tiles = AsyncPmTilesReader::try_from_source(backend).await.unwrap();
        // Verify that the test case does contain a leaf directory
        assert_ne!(0, tiles.get_header().leaf_length);
        let tile = tiles.get_tile(coord).await.unwrap().unwrap();
        assert_eq!(tile, expected_contents);
    }

    #[cfg(feature = "brotli")]
    #[tokio::test]
    async fn brotli_decompression() {
        // This fixture has brotli internal and tile compression, and one tile 0_0_0.png.
        let backend = FileBackend::try_from("fixtures/single_tile_brotli.pmtiles")
            .await
            .unwrap();
        let tiles = AsyncPmTilesReader::try_from_source(backend).await.unwrap();
        let tile = tiles
            .get_tile_decompressed(TileCoord::new(0, 0, 0).unwrap())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(tiles.header.internal_compression, Compression::Brotli);
        assert_eq!(tiles.header.tile_compression, Compression::Brotli);
        assert_eq!(tile.as_ref(), include_bytes!("../fixtures/0_0_0.png"));
    }

    #[tokio::test]
    #[cfg(feature = "iter")]
    async fn entries() {
        use futures_util::TryStreamExt as _;

        let backend = FileBackend::try_from(VECTOR_FILE).await.unwrap();
        let tiles = AsyncPmTilesReader::try_from_source(backend).await.unwrap();

        let entries = Arc::new(tiles).entries();

        let all_entries: Vec<_> = entries.try_collect().await.unwrap();
        assert_eq!(all_entries.len(), 108);
    }

    /// Opens `leaf.pmtiles` with one header field changed, and returns the entry of its
    /// last tile in the data section.
    #[cfg(feature = "iter")]
    async fn open_with_header_field(
        field: usize,
        value: fn(u64) -> u64,
    ) -> (Arc<AsyncPmTilesReader<VersionedBackend>>, DirEntry) {
        use futures_util::TryStreamExt as _;

        let data: &[u8] = include_bytes!("../fixtures/leaf.pmtiles");
        let reader = AsyncPmTilesReader::try_from_source(VersionedBackend::new(data, None))
            .await
            .unwrap();
        let entries: Vec<_> = Arc::new(reader).entries().try_collect().await.unwrap();
        let last = entries
            .into_iter()
            .max_by_key(|entry| entry.offset + u64::from(entry.length))
            .unwrap();
        assert!(last.offset > 0);

        let mut data = data.to_vec();
        let value = value(last.offset + u64::from(last.length));
        data[field..field + 8].copy_from_slice(&value.to_le_bytes());
        let reader = AsyncPmTilesReader::try_from_source(VersionedBackend::new(&data, None))
            .await
            .unwrap();
        (Arc::new(reader), last)
    }

    /// A tile or leaf directory outside its section is an invalid entry, not a panic,
    /// a wrapped offset, or a read of another section's bytes.
    #[rstest]
    #[case::tile_past_data_section(DATA_LENGTH, |tile_end| tile_end - 1)]
    #[case::leaf_past_leaf_section(LEAF_LENGTH, |_| 0)]
    #[tokio::test]
    #[cfg(feature = "iter")]
    async fn rejects_entry_outside_its_section(
        #[case] field: usize,
        #[case] value: fn(u64) -> u64,
    ) {
        let (reader, last) = open_with_header_field(field, value).await;

        let tile = reader.get_tile(TileId::new(last.tile_id).unwrap()).await;
        assert!(matches!(tile, Err(PmtError::InvalidEntry)), "{tile:?}");
    }

    /// Listing entries rejects a leaf directory outside the leaf section.
    #[tokio::test]
    #[cfg(feature = "iter")]
    async fn entries_rejects_leaf_outside_leaf_section() {
        use futures_util::TryStreamExt as _;

        let (reader, _) = open_with_header_field(LEAF_LENGTH, |_| 0).await;

        let entries = reader.entries().try_collect::<Vec<_>>().await;
        assert!(
            matches!(entries, Err(PmtError::InvalidEntry)),
            "{entries:?}"
        );
    }

    /// Returns one byte more than requested.
    struct TooLongBackend;

    impl AsyncBackend for TooLongBackend {
        fn read(
            &self,
            _offset: u64,
            length: usize,
        ) -> impl Future<Output = PmtResult<BackendResponse>> + Send {
            future::ready(Ok(BackendResponse::new(Bytes::from(vec![0; length + 1]))))
        }
    }

    /// The reader rejects a backend that returns more than it asked for, whatever the backend.
    #[tokio::test]
    async fn rejects_too_long_response() {
        let result = AsyncPmTilesReader::try_from_source(TooLongBackend).await;
        assert!(matches!(
            result,
            Err(PmtError::UnexpectedNumberOfBytesReturned(16_384, 16_385))
        ));
    }

    /// Serves an archive with everything after the root directory moved 4 GiB further,
    /// so that all later reads are above `u32::MAX`.
    #[cfg(feature = "iter")]
    struct ShiftedBackend(Bytes);

    #[cfg(feature = "iter")]
    impl ShiftedBackend {
        const SHIFT: u64 = 1 << 32;

        fn new(data: &[u8]) -> Self {
            let mut data = data.to_vec();
            for field in [METADATA_OFFSET, LEAF_OFFSET, DATA_OFFSET] {
                let bytes = &mut data[field..field + 8];
                let offset = u64::from_le_bytes(bytes.try_into().unwrap());
                bytes.copy_from_slice(&(offset + Self::SHIFT).to_le_bytes());
            }
            Self(data.into())
        }
    }

    #[cfg(feature = "iter")]
    impl AsyncBackend for ShiftedBackend {
        fn read(
            &self,
            offset: u64,
            length: usize,
        ) -> impl Future<Output = PmtResult<BackendResponse>> + Send {
            // Only the initial read of the header and root directory is below the shift.
            let start = if offset == 0 {
                Some(0)
            } else {
                offset.checked_sub(Self::SHIFT)
            };
            let response = match start {
                Some(start) => {
                    let start = usize::try_from(start).unwrap();
                    let end = (start + length).min(self.0.len());
                    Ok(BackendResponse::new(self.0.slice(start..end)))
                }
                None => Err(PmtError::InvalidEntry),
            };
            future::ready(response)
        }
    }

    /// Metadata, leaf directories and tiles above 4 GiB are read from their full offsets.
    /// Offsets only truncate on 32-bit targets, so this can only fail there.
    #[tokio::test]
    #[cfg(feature = "iter")]
    async fn reads_offsets_above_4_gib() {
        use futures_util::TryStreamExt as _;

        let data: &[u8] = include_bytes!("../fixtures/leaf.pmtiles");
        let expected = AsyncPmTilesReader::try_from_source(VersionedBackend::new(data, None))
            .await
            .unwrap();
        let expected = Arc::new(expected);
        let shifted = AsyncPmTilesReader::try_from_source_and_cache(
            ShiftedBackend::new(data),
            HashMapCache::default(),
        )
        .await
        .unwrap();
        let shifted = Arc::new(shifted);

        assert_eq!(
            shifted.get_metadata().await.unwrap(),
            expected.get_metadata().await.unwrap()
        );

        let entries: Vec<_> = Arc::clone(&expected).entries().try_collect().await.unwrap();
        let shifted_entries: Vec<_> = Arc::clone(&shifted).entries().try_collect().await.unwrap();
        assert_eq!(entries.len(), 5);
        assert_eq!(shifted_entries, entries);

        for entry in entries {
            let tile_id = TileId::new(entry.tile_id).unwrap();
            let tile = shifted.get_tile(tile_id).await.unwrap();
            assert!(tile.is_some());
            assert_eq!(tile, expected.get_tile(tile_id).await.unwrap());
        }
    }

    /// Builds an uncompressed archive whose root directory lists one leaf directory, which
    /// lists itself.
    #[cfg(feature = "write")]
    fn archive_with_self_listing_leaf() -> Vec<u8> {
        use crate::writer::WriteTo as _;
        use crate::{DirEntry, Directory, Header, TileType};

        let leaf_entry = |length| DirEntry {
            tile_id: 0,
            offset: 0,
            length,
            run_length: 0,
        };
        let mut leaf = Vec::new();
        // One entry takes 5 bytes: the entry count, and one byte for each of its four fields
        Directory::from_entries(vec![leaf_entry(5)])
            .write_to(&mut leaf)
            .unwrap();
        assert_eq!(leaf.len(), 5);
        let mut root = Vec::new();
        Directory::from_entries(vec![leaf_entry(5)])
            .write_to(&mut root)
            .unwrap();
        let metadata = b"{}";

        let mut header = Header::new(Compression::None, TileType::Mvt);
        header.internal_compression = Compression::None;
        header.root_length = root.len() as u64;
        header.metadata_offset = header.root_offset + header.root_length;
        header.metadata_length = metadata.len() as u64;
        header.leaf_offset = header.metadata_offset + header.metadata_length;
        header.leaf_length = leaf.len() as u64;
        header.data_offset = header.leaf_offset + header.leaf_length;

        let mut data = Vec::new();
        header.write_to(&mut data).unwrap();
        data.extend([root.as_slice(), metadata, &leaf].concat());
        data
    }

    /// A tile lookup through leaf directories that never end is an invalid entry.
    #[tokio::test]
    #[cfg(feature = "write")]
    async fn get_tile_rejects_self_listing_leaf() {
        let data = archive_with_self_listing_leaf();
        let reader = AsyncPmTilesReader::try_from_source(VersionedBackend::new(&data, None))
            .await
            .unwrap();

        let tile = reader.get_tile(id(0, 0, 0)).await;
        assert!(matches!(tile, Err(PmtError::InvalidEntry)), "{tile:?}");
    }

    /// Listing entries rejects a leaf directory it already read, instead of looping forever.
    #[tokio::test]
    #[cfg(all(feature = "write", feature = "iter"))]
    async fn entries_rejects_self_listing_leaf() {
        use futures_util::TryStreamExt as _;

        let data = archive_with_self_listing_leaf();
        let reader = AsyncPmTilesReader::try_from_source(VersionedBackend::new(&data, None))
            .await
            .unwrap();

        let entries = Arc::new(reader).entries().try_collect::<Vec<_>>().await;
        assert!(
            matches!(entries, Err(PmtError::InvalidEntry)),
            "{entries:?}"
        );
    }
}
