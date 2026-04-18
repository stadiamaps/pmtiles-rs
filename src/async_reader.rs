// FIXME: This seems like a bug - there are lots of u64 to usize conversions in this file,
//        so any file larger than 4GB, or an untrusted file with bad data may crash.
#![expect(clippy::cast_possible_truncation)]

use std::future::Future;
#[cfg(feature = "iter-async")]
use std::sync::Arc;

#[cfg(feature = "iter-async")]
use async_stream::try_stream;
use bytes::Bytes;
#[cfg(feature = "iter-async")]
use futures_util::stream::BoxStream;
#[cfg(feature = "__async")]
use tokio::io::AsyncReadExt as _;

use crate::PmtError::UnsupportedCompression;
use crate::header::{HEADER_SIZE, MAX_INITIAL_BYTES};
use crate::{Compression, DirEntry, Directory, Header, PmtError, PmtResult, TileId};
#[cfg(feature = "__async")]
use crate::{DirectoryCache, NoCache};

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

/// An asynchronous reader for `PMTiles` archives.
pub struct AsyncPmTilesReader<B, C = NoCache> {
    backend: B,
    cache: C,
    header: Header,
    root_directory: Directory,
    initial_data_version_string: Option<String>,
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
        Self::try_from_cached_source(backend, NoCache).await
    }
}

impl<B: AsyncBackend + Sync + Send, C: DirectoryCache + Sync + Send> AsyncPmTilesReader<B, C> {
    /// Creates a new cached reader from a specified source and validates the provided `PMTiles` archive is valid.
    ///
    /// Note: Prefer using `new_with_*` methods.
    ///
    /// # Errors
    ///
    /// This function will return an error if
    /// - the backend fails to read the header/root directory,
    /// - or if the root directory is malformed
    pub async fn try_from_cached_source(backend: B, cache: C) -> PmtResult<Self> {
        // Read the first 127 and up to 16,384 bytes to ensure we can initialize the header and root directory.
        let initial_response = backend.read(0, MAX_INITIAL_BYTES).await?;
        let mut initial_bytes = initial_response.bytes;
        let initial_data_version_string = initial_response.data_version_string;
        if initial_bytes.len() < HEADER_SIZE {
            return Err(PmtError::InvalidHeader);
        }

        let header = Header::try_from_bytes(initial_bytes.split_to(HEADER_SIZE))?;

        let directory_bytes = initial_bytes
            .split_off((header.root_offset as usize) - HEADER_SIZE)
            .split_to(header.root_length as _);

        let root_directory =
            Self::read_compressed_directory(header.internal_compression, directory_bytes).await?;

        Ok(Self {
            backend,
            cache,
            header,
            root_directory,
            initial_data_version_string,
        })
    }

    /// Fetches tile data using either [`TileCoord`](crate::TileCoord) or [`TileId`] to locate the tile.
    ///
    /// ```no_run
    /// # async fn test() {
    /// #     let backend = pmtiles::MmapBackend::try_from("").await.unwrap();
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

        let offset = (self.header.data_offset + entry.offset) as _;
        let length = entry.length as _;
        let backend_response = self.backend.read_exact(offset, length).await?;

        // Compare the initial data version string (stored at instantiation time based on the `PMTiles` metadata)
        // against the latest version string, but only if both are set.
        //
        // If an initial data version string was not available, the backend does not support exposing version strings.
        // If a current data version string is not available, this request was unable to extract a data version string,
        // and the check should be skipped.
        if let (Some(expected), Some(actual)) = (
            &self.initial_data_version_string,
            &backend_response.data_version_string,
        ) && expected != actual
        {
            return Err(PmtError::SourceModified);
        }

        Ok(Some(backend_response.bytes))
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
            Some(Self::decompress(self.header.tile_compression, data).await?)
        } else {
            None
        })
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
    /// - backend fails to read the metadata or
    /// - metadata cannot be decompressed or
    /// - is not valid UTF-8
    pub async fn get_metadata(&self) -> PmtResult<String> {
        let offset = self.header.metadata_offset as _;
        let length = self.header.metadata_length as _;
        let response = self.backend.read_exact(offset, length).await?;
        let metadata = response.bytes;

        let decompressed_metadata =
            Self::decompress(self.header.internal_compression, metadata).await?;

        Ok(String::from_utf8(decompressed_metadata.to_vec())?)
    }

    /// Return an async stream over all tile entries in the archive. Directory entries are traversed, and not included in the result.
    /// Because this function requires the reader for the duration of the stream, you need to wrap the reader in an `Arc`.
    ///
    /// ```
    /// # use std::sync::Arc;
    /// # use pmtiles::{AsyncPmTilesReader, MmapBackend};
    /// # use futures_util::TryStreamExt as _;
    /// #[tokio::main(flavor="current_thread")]
    /// async fn main() -> Result<(), pmtiles::PmtError> {
    ///     let backend = MmapBackend::try_from("fixtures/protomaps(vector)ODbL_firenze.pmtiles").await?;
    ///     let reader = Arc::new(AsyncPmTilesReader::try_from_source(backend).await?);
    ///     let mut entries = reader.entries();
    ///     while let Some(entry) = entries.try_next().await? {
    ///        // ... do something with entry ...
    ///     }
    ///     Ok(())
    /// }
    /// ```
    #[cfg(feature = "iter-async")]
    pub fn entries<'a>(self: Arc<Self>) -> BoxStream<'a, PmtResult<DirEntry>>
    where
        B: 'a,
        C: 'a,
    {
        Box::pin(try_stream! {
            let mut queue = std::collections::VecDeque::new();

            for entry in &self.root_directory.entries {
                queue.push_back(entry.clone());
            }

            while let Some(entry) = queue.pop_front() {
                if entry.is_leaf() {
                    let offset = (self.header.leaf_offset + entry.offset) as _;
                    let length = entry.length as usize;
                    let leaf_dir = self.read_directory(offset, length).await?;
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
    pub async fn parse_tilejson(&self, sources: Vec<String>) -> PmtResult<tilejson::TileJSON> {
        use serde_json::Value;

        let meta = self.get_metadata().await?;
        let meta: Value = serde_json::from_str(&meta).map_err(|_| PmtError::InvalidMetadata)?;
        let Value::Object(meta) = meta else {
            return Err(PmtError::InvalidMetadata);
        };

        let mut tj = self.header.get_tilejson(sources);
        for (key, value) in meta {
            if let Value::String(v) = value {
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
                } else if key == "minzoom" || key == "maxzoom" {
                    // We already have the correct values from the header, so just drop these
                    // attributes from the metadata silently, don't overwrite known-good values.
                } else {
                    tj.other.insert(key, Value::String(v));
                }
            } else if key == "vector_layers" {
                if let Ok(v) = serde_json::from_value::<Vec<tilejson::VectorLayer>>(value) {
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

        Ok(entry.cloned())
    }

    async fn find_entry_rec(
        &self,
        tile_id: TileId,
        entry: &DirEntry,
        depth: u8,
    ) -> PmtResult<Option<DirEntry>> {
        // the recursion is done as two functions because it is a bit cleaner,
        // and it allows the directory to be cached later without cloning it first.
        let offset = (self.header.leaf_offset + entry.offset) as _;

        let entry = self
            .cache
            .get_dir_entry_or_insert(offset, tile_id, async move {
                let length = entry.length as _;
                self.read_directory(offset, length).await
            })
            .await?;

        if let Some(ref entry) = entry
            && entry.is_leaf()
        {
            return if depth <= 4 {
                Box::pin(self.find_entry_rec(tile_id, entry, depth + 1)).await
            } else {
                Ok(None)
            };
        }

        Ok(entry)
    }

    async fn read_directory(&self, offset: usize, length: usize) -> PmtResult<Directory> {
        let data = self.backend.read_exact(offset, length).await?;
        Self::read_compressed_directory(self.header.internal_compression, data.bytes).await
    }

    async fn read_compressed_directory(
        compression: Compression,
        bytes: Bytes,
    ) -> PmtResult<Directory> {
        let decompressed_bytes = Self::decompress(compression, bytes).await?;
        Directory::try_from(decompressed_bytes)
    }

    async fn decompress(compression: Compression, bytes: Bytes) -> PmtResult<Bytes> {
        if compression == Compression::None {
            return Ok(bytes);
        }

        let mut decompressed_bytes = Vec::with_capacity(bytes.len() * 2);
        match compression {
            Compression::Gzip => {
                async_compression::tokio::bufread::GzipDecoder::new(&bytes[..])
                    .read_to_end(&mut decompressed_bytes)
                    .await?;
            }
            #[cfg(feature = "brotli")]
            Compression::Brotli => {
                async_compression::tokio::bufread::BrotliDecoder::new(&bytes[..])
                    .read_to_end(&mut decompressed_bytes)
                    .await?;
            }
            #[cfg(feature = "zstd")]
            Compression::Zstd => {
                async_compression::tokio::bufread::ZstdDecoder::new(&bytes[..])
                    .read_to_end(&mut decompressed_bytes)
                    .await?;
            }
            Compression::None => {
                return Ok(bytes);
            }
            v => Err(UnsupportedCompression(v))?,
        }

        Ok(Bytes::from(decompressed_bytes))
    }
}

/// A trait for asynchronous backends that can read data from a `PMTiles` source.
pub trait AsyncBackend {
    /// Reads exactly `length` bytes starting at `offset`
    fn read_exact(
        &self,
        offset: usize,
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
    fn read(
        &self,
        offset: usize,
        length: usize,
    ) -> impl Future<Output = PmtResult<BackendResponse>> + Send;
}

#[cfg(test)]
#[cfg(feature = "mmap-async-tokio")]
mod tests {
    use rstest::rstest;

    use crate::tests::{RASTER_FILE, VECTOR_FILE};
    use crate::{AsyncPmTilesReader, MmapBackend, TileCoord};

    fn id(z: u8, x: u32, y: u32) -> TileCoord {
        TileCoord::new(z, x, y).unwrap()
    }

    #[rstest]
    #[case(RASTER_FILE)]
    #[case(VECTOR_FILE)]
    #[tokio::test]
    async fn open_sanity_check(#[case] file: &str) {
        let backend = MmapBackend::try_from(file).await.unwrap();
        AsyncPmTilesReader::try_from_source(backend).await.unwrap();
    }

    #[rstest]
    #[case(id(0, 0, 0), include_bytes!("../fixtures/0_0_0.png"))]
    #[case(id(2, 2, 2), include_bytes!("../fixtures/2_2_2.png"))]
    #[case(id(3, 4, 5), include_bytes!("../fixtures/3_4_5.png"))]
    #[tokio::test]
    async fn get_tiles(#[case] coord: TileCoord, #[case] fixture_bytes: &[u8]) {
        let backend = MmapBackend::try_from(RASTER_FILE).await.unwrap();
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
        use object_store::{ObjectStoreExt, WriteMultipart};

        let store = Box::new(object_store::memory::InMemory::new());

        let upload_path = object_store::path::Path::from("file.pmtiles");
        let upload = store.put_multipart(&upload_path).await.unwrap();
        let mut write = WriteMultipart::new(upload);
        write.write(include_bytes!(
            "../fixtures/stamen_toner(raster)CC-BY+ODbL_z3.pmtiles"
        ));
        write.finish().await.unwrap();
        let backend = crate::ObjectStoreBackend::new(store, upload_path);
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
    async fn test_data_version_source_modified() {
        // Tests that if the underlying storage object changes after initial read of a
        // pmtiles header, a subsequent read will fail.
        use std::sync::Arc;

        use object_store::path::Path;
        use object_store::{ObjectStore, PutOptions, PutPayload};

        // The test uses the ObjectStoreBackend with an InMemory underlying store since that
        // makes it easy to modify the ETag of a stored object by putting a new "version".
        use crate::ObjectStoreBackend;

        // The exact file here does not matter - it is only important that we fetch a valid
        // tile below.
        let data: &[u8] = include_bytes!("../fixtures/stamen_toner(raster)CC-BY+ODbL_z3.pmtiles");
        let store = Arc::new(object_store::memory::InMemory::new());
        let path = Path::from("file.pmtiles");

        let payload: PutPayload = bytes::Bytes::copy_from_slice(data).into();
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
        let payload: PutPayload = bytes::Bytes::copy_from_slice(data).into();
        store
            .put_opts(&path, payload, PutOptions::default())
            .await
            .unwrap();

        let result = tiles.get_tile(id(0, 0, 0)).await;
        assert!(matches!(result, Err(crate::PmtError::SourceModified)));
    }

    #[rstest]
    #[case(id(6, 31, 23), false)] // missing tile
    #[case(id(12, 2174, 1492), true)] // existing leaf tile
    #[tokio::test]
    async fn test_tile_existence(#[case] coord: TileCoord, #[case] should_exist: bool) {
        let backend = MmapBackend::try_from(VECTOR_FILE).await.unwrap();
        let tiles = AsyncPmTilesReader::try_from_source(backend).await.unwrap();

        let tile = tiles.get_tile(coord).await;
        assert!(tile.is_ok());

        if should_exist {
            assert!(tile.unwrap().is_some());
        } else {
            assert!(tile.unwrap().is_none());
        }
    }

    #[tokio::test]
    async fn test_leaf_tile_compressed() {
        let backend = MmapBackend::try_from(VECTOR_FILE).await.unwrap();
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
    async fn test_get_metadata() {
        let backend = MmapBackend::try_from(VECTOR_FILE).await.unwrap();
        let tiles = AsyncPmTilesReader::try_from_source(backend).await.unwrap();

        let metadata = tiles.get_metadata().await.unwrap();
        assert!(!metadata.is_empty());
    }

    #[rstest]
    #[case(VECTOR_FILE, |tj: &tilejson::TileJSON| assert!(tj.attribution.is_some()))]
    #[case(RASTER_FILE, |tj: &tilejson::TileJSON| assert!(tj.other.is_empty()))]
    #[tokio::test]
    #[cfg(feature = "tilejson")]
    async fn test_parse_tilejson(
        #[case] file: &str,
        #[case] assertion: impl Fn(&tilejson::TileJSON),
    ) {
        let backend = MmapBackend::try_from(file).await.unwrap();
        let tiles = AsyncPmTilesReader::try_from_source(backend).await.unwrap();
        let tj = tiles.parse_tilejson(Vec::new()).await.unwrap();
        assertion(&tj);
    }

    #[rstest]
    #[case(id(0, 0, 0), b"0")]
    #[case(id(1, 0, 0), b"1")]
    #[case(id(1, 0, 1), b"2")]
    #[case(id(1, 1, 1), b"3")]
    #[case(id(1, 1, 0), b"4")]
    #[tokio::test]
    async fn test_martin_675(#[case] coord: TileCoord, #[case] expected_contents: &[u8]) {
        let backend = MmapBackend::try_from("fixtures/leaf.pmtiles")
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
    async fn test_brotli_decompression() {
        // This fixture has brotli internal and tile compression, and one tile 0_0_0.png.
        let backend = MmapBackend::try_from("fixtures/single_tile_brotli.pmtiles")
            .await
            .unwrap();
        let tiles = AsyncPmTilesReader::try_from_source(backend).await.unwrap();
        let tile = tiles
            .get_tile_decompressed(TileCoord::new(0, 0, 0).unwrap())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(
            tiles.header.internal_compression,
            crate::Compression::Brotli
        );
        assert_eq!(tiles.header.tile_compression, crate::Compression::Brotli);
        assert_eq!(tile.as_ref(), include_bytes!("../fixtures/0_0_0.png"));
    }

    #[tokio::test]
    #[cfg(feature = "iter-async")]
    async fn test_entries() {
        use futures_util::TryStreamExt as _;

        let backend = MmapBackend::try_from(VECTOR_FILE).await.unwrap();
        let tiles = AsyncPmTilesReader::try_from_source(backend).await.unwrap();

        let entries = std::sync::Arc::new(tiles).entries();

        let all_entries: Vec<_> = entries.try_collect().await.unwrap();
        assert_eq!(all_entries.len(), 108);
    }
}
