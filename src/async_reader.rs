#[cfg(feature = "mmap-async-tokio")]
use std::path::Path;

use async_recursion::async_recursion;
use async_trait::async_trait;
use bytes::Bytes;
#[cfg(feature = "http-async")]
use reqwest::{Client, IntoUrl};
#[cfg(feature = "tokio")]
use tokio::io::AsyncReadExt;

use crate::directory::{Directory, Entry};
use crate::error::Error;
use crate::header::{HEADER_SIZE, MAX_INITIAL_BYTES};
#[cfg(feature = "http-async")]
use crate::http::HttpBackend;
#[cfg(feature = "mmap-async-tokio")]
use crate::mmap::MmapBackend;
use crate::tile::{tile_id, Tile};
use crate::{Compression, Header};

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

    #[cfg(feature = "tilejson")]
    pub async fn parse_tilejson(&self, sources: Vec<String>) -> Result<tilejson::TileJSON, Error> {
        use serde_json::Value;

        let meta = self.get_metadata().await?;
        let meta: Value = serde_json::from_str(&meta).map_err(|_| Error::InvalidMetadata)?;
        let Value::Object(meta) = meta else {
            return Err(Error::InvalidMetadata);
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
                    return Err(Error::InvalidMetadata);
                }
            } else {
                tj.other.insert(key, value);
            }
        }
        Ok(tj)
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
    pub async fn new_with_path<P: AsRef<Path>>(p: P) -> Result<Self, Error> {
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
#[cfg(feature = "mmap-async-tokio")]
mod tests {
    use super::AsyncPmTilesReader;
    use crate::mmap::MmapBackend;
    use crate::tests::{RASTER_FILE, VECTOR_FILE};

    #[tokio::test]
    async fn open_sanity_check() {
        let backend = MmapBackend::try_from(RASTER_FILE).await.unwrap();
        AsyncPmTilesReader::try_from_source(backend).await.unwrap();
    }

    async fn compare_tiles(z: u8, x: u64, y: u64, fixture_bytes: &[u8]) {
        let backend = MmapBackend::try_from(RASTER_FILE).await.unwrap();
        let tiles = AsyncPmTilesReader::try_from_source(backend).await.unwrap();
        let tile = tiles.get_tile(z, x, y).await.unwrap();

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
        let backend = MmapBackend::try_from(VECTOR_FILE).await.unwrap();
        let tiles = AsyncPmTilesReader::try_from_source(backend).await.unwrap();

        let tile = tiles.get_tile(6, 31, 23).await;
        assert!(tile.is_none());
    }

    #[tokio::test]
    async fn test_leaf_tile() {
        let backend = MmapBackend::try_from(VECTOR_FILE).await.unwrap();
        let tiles = AsyncPmTilesReader::try_from_source(backend).await.unwrap();

        let tile = tiles.get_tile(12, 2174, 1492).await;
        assert!(tile.is_some());
    }

    #[tokio::test]
    async fn test_get_metadata() {
        let backend = MmapBackend::try_from(VECTOR_FILE).await.unwrap();
        let tiles = AsyncPmTilesReader::try_from_source(backend).await.unwrap();

        let metadata = tiles.get_metadata().await.unwrap();
        assert!(!metadata.is_empty());
    }

    #[tokio::test]
    #[cfg(feature = "tilejson")]
    async fn test_parse_tilejson() {
        let backend = MmapBackend::try_from(VECTOR_FILE).await.unwrap();
        let tiles = AsyncPmTilesReader::try_from_source(backend).await.unwrap();

        let tj = tiles.parse_tilejson(Vec::new()).await.unwrap();
        assert!(tj.attribution.is_some());
    }

    #[tokio::test]
    #[cfg(feature = "tilejson")]
    async fn test_parse_tilejson2() {
        let backend = MmapBackend::try_from(RASTER_FILE).await.unwrap();
        let tiles = AsyncPmTilesReader::try_from_source(backend).await.unwrap();

        let tj = tiles.parse_tilejson(Vec::new()).await.unwrap();
        assert!(tj.other.is_empty());
    }

    #[tokio::test]
    #[ignore = "This test requires a 200mb file to be downloaded. See https://github.com/maplibre/martin/issues/675"]
    async fn test_martin_675() {
        // the file was manually placed here from the test because it is 200mb
        // see also https://github.com/protomaps/PMTiles/issues/182 - once the file is shrunk somehow?
        let backend = MmapBackend::try_from("fixtures/tiles.pmtiles")
            .await
            .unwrap();
        let tiles = AsyncPmTilesReader::try_from_source(backend).await.unwrap();
        let tile = tiles.get_tile(7, 35, 50).await;
        assert!(tile.is_some());
    }
}
