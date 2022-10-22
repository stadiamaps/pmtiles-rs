#[cfg(feature = "mmap-async-tokio")]
use std::path::Path;

use async_recursion::async_recursion;
use async_trait::async_trait;
use bytes::Bytes;
#[cfg(feature = "http-async")]
use reqwest::{Client, IntoUrl};
#[cfg(feature = "tokio")]
use tokio::io::AsyncReadExt;

use crate::header::{HEADER_SIZE, MAX_INITIAL_BYTES};
#[cfg(feature = "http-async")]
use crate::http::HttpBackend;
#[cfg(feature = "mmap-async-tokio")]
use crate::mmap::MmapBackend;
use crate::{
    directory::{Directory, Entry},
    error::Error,
    tile::{tile_id, Tile},
    Compression, Header,
};

pub struct AsyncPmTilesReader<B: AsyncBackend> {
    pub header: Header,
    backend: B,
    root_directory: Directory,
}

impl<B: AsyncBackend + Sync + Send> AsyncPmTilesReader<B> {
    /// Creates a new reader from a specified source and validates the provided PMTiles archive is valid.
    ///
    /// Note: Prefer using new_with_* methods.
    pub async fn try_from_source(backend: B) -> Result<Self, Error> {
        let mut initial_bytes = backend.read_initial_bytes().await?;

        let header_bytes = initial_bytes.split_to(HEADER_SIZE);

        let header = Header::try_from_bytes(header_bytes)?;

        let directory_bytes = initial_bytes
            .split_off((header.root_offset as usize) - HEADER_SIZE)
            .split_to(header.root_length as _);

        let root_directory =
            Self::read_compressed_directory(header.internal_compression, directory_bytes).await?;

        Ok(Self {
            header,
            backend,
            root_directory,
        })
    }

    /// Fetches a [Tile] from the archive.
    pub async fn get_tile(&self, z: u8, x: u64, y: u64) -> Option<Tile> {
        let tile_id = tile_id(z, x, y);
        let entry = self.find_tile_entry(tile_id, None, 0).await?;

        let data = self
            .backend
            .read_exact(
                (self.header.data_offset + entry.offset) as _,
                entry.length as _,
            )
            .await
            .ok()?;

        Some(Tile {
            data,
            tile_type: self.header.tile_type,
            tile_compression: self.header.tile_compression,
        })
    }

    /// Gets metadata from the archive.
    ///
    /// Note: by spec, this should be valid JSON. This method currently returns a [String].
    /// This may change in the future.
    pub async fn get_metadata(&self) -> Result<String, Error> {
        let metadata = self
            .backend
            .read_exact(
                self.header.metadata_offset as _,
                self.header.metadata_length as _,
            )
            .await?;

        let decompressed_metadata =
            Self::decompress(self.header.internal_compression, metadata).await?;

        Ok(String::from_utf8(decompressed_metadata.to_vec())?)
    }

    #[async_recursion]
    async fn find_tile_entry(
        &self,
        tile_id: u64,
        next_dir: Option<Directory>,
        depth: u8,
    ) -> Option<Entry> {
        // Max recursion...
        if depth >= 4 {
            return None;
        }

        let next_dir = next_dir.as_ref().unwrap_or(&self.root_directory);

        match next_dir.find_tile_id(tile_id) {
            None => None,
            Some(needle) => {
                if needle.run_length == 0 {
                    // Leaf directory
                    let next_dir = self
                        .read_directory(
                            (self.header.leaf_offset + needle.offset) as _,
                            needle.length as _,
                        )
                        .await
                        .ok()?;
                    self.find_tile_entry(tile_id, Some(next_dir), depth + 1)
                        .await
                } else {
                    Some(needle.clone())
                }
            }
        }
    }

    async fn read_directory(&self, offset: usize, length: usize) -> Result<Directory, Error> {
        Self::read_directory_with_backend(
            &self.backend,
            self.header.internal_compression,
            offset,
            length,
        )
        .await
    }

    async fn read_compressed_directory(
        compression: Compression,
        bytes: Bytes,
    ) -> Result<Directory, Error> {
        let decompressed_bytes = Self::decompress(compression, bytes).await?;

        Directory::try_from(decompressed_bytes)
    }

    async fn read_directory_with_backend(
        backend: &B,
        compression: Compression,
        offset: usize,
        length: usize,
    ) -> Result<Directory, Error> {
        let directory_bytes = backend.read_exact(offset, length).await?;

        Self::read_compressed_directory(compression, directory_bytes).await
    }

    async fn decompress(compression: Compression, bytes: Bytes) -> Result<Bytes, Error> {
        let mut decompressed_bytes = Vec::with_capacity(bytes.len() * 2);
        match compression {
            Compression::Gzip => {
                async_compression::tokio::bufread::GzipDecoder::new(&bytes[..])
                    .read_to_end(&mut decompressed_bytes)
                    .await?;
            }
            _ => todo!("Support other forms of internal compression."),
        }

        Ok(Bytes::from(decompressed_bytes))
    }
}

#[cfg(feature = "http-async")]
impl AsyncPmTilesReader<HttpBackend> {
    /// Creates a new PMTiles reader from a URL using the Reqwest backend.
    ///
    /// Fails if [url] does not exist or is an invalid archive. (Note: HTTP requests are made to validate it.)
    pub async fn new_with_url<U: IntoUrl>(client: Client, url: U) -> Result<Self, Error> {
        let backend = HttpBackend::try_from(client, url)?;

        Self::try_from_source(backend).await
    }
}

#[cfg(feature = "mmap-async-tokio")]
impl AsyncPmTilesReader<MmapBackend> {
    /// Creates a new PMTiles reader from a file path using the async mmap backend.
    ///
    /// Fails if [p] does not exist or is an invalid archive.
    pub async fn new_with_path(p: &Path) -> Result<Self, Error> {
        let backend = MmapBackend::try_from(p).await?;

        Self::try_from_source(backend).await
    }
}

#[async_trait]
pub trait AsyncBackend {
    /// Reads exactly `length` bytes starting at `offset`
    async fn read_exact(&self, offset: usize, length: usize) -> Result<Bytes, Error>;

    /// Reads up to `length` bytes starting at `offset`.
    async fn read(&self, offset: usize, length: usize) -> Result<Bytes, Error>;

    /// Read the first 127 and up to 16,384 bytes to ensure we can initialize the header and root directory.
    async fn read_initial_bytes(&self) -> Result<Bytes, Error> {
        let bytes = self.read(0, MAX_INITIAL_BYTES).await?;
        if bytes.len() < HEADER_SIZE {
            return Err(Error::InvalidHeader);
        }

        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::mmap::MmapBackend;

    use super::AsyncPmTilesReader;

    async fn create_backend() -> MmapBackend {
        MmapBackend::try_from(Path::new(
            "fixtures/stamen_toner(raster)CC-BY+ODbL_z3.pmtiles",
        ))
        .await
        .expect("Unable to open test file.")
    }

    #[tokio::test]
    async fn open_sanity_check() {
        let backend = create_backend().await;
        AsyncPmTilesReader::try_from_source(backend)
            .await
            .expect("Unable to open PMTiles");
    }

    async fn compare_tiles(z: u8, x: u64, y: u64, fixture_bytes: &[u8]) {
        let backend = create_backend().await;
        let tiles = AsyncPmTilesReader::try_from_source(backend)
            .await
            .expect("Unable to open PMTiles");

        let tile = tiles
            .get_tile(z, x, y)
            .await
            .expect("Expected to get a tile.");

        assert_eq!(
            tile.data.len(),
            fixture_bytes.len(),
            "Expected tile length to match."
        );
        assert_eq!(tile.data, fixture_bytes, "Expected tile to match fixture.");
    }

    #[tokio::test]
    async fn get_first_tile() {
        let fixture_tile = include_bytes!("../fixtures/0_0_0.png");
        compare_tiles(0, 0, 0, fixture_tile).await;
    }

    #[tokio::test]
    async fn get_another_tile() {
        let fixture_tile = include_bytes!("../fixtures/2_2_2.png");
        compare_tiles(2, 2, 2, fixture_tile).await;
    }

    #[tokio::test]
    async fn get_yet_another_tile() {
        let fixture_tile = include_bytes!("../fixtures/3_4_5.png");
        compare_tiles(3, 4, 5, fixture_tile).await;
    }

    #[tokio::test]
    async fn test_missing_tile() {
        let backend =
            MmapBackend::try_from(Path::new("fixtures/protomaps(vector)ODbL_firenze.pmtiles"))
                .await
                .expect("Unable to open test file.");
        let tiles = AsyncPmTilesReader::try_from_source(backend)
            .await
            .expect("Unable to open PMTiles");

        let tile = tiles.get_tile(6, 31, 23).await;
        assert!(tile.is_none());
    }

    #[tokio::test]
    async fn test_leaf_tile() {
        let backend =
            MmapBackend::try_from(Path::new("fixtures/protomaps(vector)ODbL_firenze.pmtiles"))
                .await
                .expect("Unable to open test file.");
        let tiles = AsyncPmTilesReader::try_from_source(backend)
            .await
            .expect("Unable to open PMTiles");

        let tile = tiles.get_tile(12, 2174, 1492).await;
        assert!(tile.is_some());
    }

    #[tokio::test]
    async fn test_get_metadata() {
        let backend =
            MmapBackend::try_from(Path::new("fixtures/protomaps(vector)ODbL_firenze.pmtiles"))
                .await
                .expect("Unable to open test file.");
        let tiles = AsyncPmTilesReader::try_from_source(backend)
            .await
            .expect("Unable to open PMTiles");

        let metadata = tiles.get_metadata().await.expect("Unable to read metadata");

        assert!(!metadata.is_empty());
    }
}
