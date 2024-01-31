// FIXME: This seems like a bug - there are lots of u64 to usize conversions in this file,
//        so any file larger than 4GB, or an untrusted file with bad data may crash.
#![allow(clippy::cast_possible_truncation)]

#[cfg(feature = "mmap-async-tokio")]
use std::path::Path;

use async_recursion::async_recursion;
use async_trait::async_trait;
use bytes::Bytes;
#[cfg(feature = "http-async")]
use reqwest::{Client, IntoUrl};
#[cfg(any(
    feature = "http-async",
    feature = "mmap-async-tokio",
    feature = "s3-async-rustls",
    feature = "s3-async-native"
))]
use tokio::io::AsyncReadExt;

use crate::cache::DirCacheResult;
#[cfg(any(
    feature = "http-async",
    feature = "mmap-async-tokio",
    feature = "s3-async-native",
    feature = "s3-async-rustls"
))]
use crate::cache::{DirectoryCache, NoCache};
use crate::directory::{DirEntry, Directory};
use crate::error::{PmtError, PmtResult};
use crate::header::{HEADER_SIZE, MAX_INITIAL_BYTES};
#[cfg(feature = "http-async")]
use crate::http::HttpBackend;
#[cfg(feature = "mmap-async-tokio")]
use crate::mmap::MmapBackend;
#[cfg(any(feature = "s3-async-rustls", feature = "s3-async-native"))]
use crate::s3::S3Backend;
use crate::tile::tile_id;
use crate::{Compression, Header};

pub struct AsyncPmTilesReader<B, C = NoCache> {
    backend: B,
    cache: C,
    header: Header,
    root_directory: Directory,
}

impl<B: AsyncBackend + Sync + Send> AsyncPmTilesReader<B, NoCache> {
    /// Creates a new reader from a specified source and validates the provided `PMTiles` archive is valid.
    ///
    /// Note: Prefer using `new_with_*` methods.
    pub async fn try_from_source(backend: B) -> PmtResult<Self> {
        Self::try_from_cached_source(backend, NoCache).await
    }
}

impl<B: AsyncBackend + Sync + Send, C: DirectoryCache + Sync + Send> AsyncPmTilesReader<B, C> {
    /// Creates a new cached reader from a specified source and validates the provided `PMTiles` archive is valid.
    ///
    /// Note: Prefer using `new_with_*` methods.
    pub async fn try_from_cached_source(backend: B, cache: C) -> PmtResult<Self> {
        // Read the first 127 and up to 16,384 bytes to ensure we can initialize the header and root directory.
        let mut initial_bytes = backend.read(0, MAX_INITIAL_BYTES).await?;
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
        })
    }

    /// Fetches tile bytes from the archive.
    pub async fn get_tile(&self, z: u8, x: u64, y: u64) -> Option<Bytes> {
        let tile_id = tile_id(z, x, y);
        let entry = self.find_tile_entry(tile_id).await?;

        let offset = (self.header.data_offset + entry.offset) as _;
        let length = entry.length as _;
        let data = self.backend.read_exact(offset, length).await.ok()?;

        Some(data)
    }

    /// Access header information.
    pub fn get_header(&self) -> &Header {
        &self.header
    }

    /// Gets metadata from the archive.
    ///
    /// Note: by spec, this should be valid JSON. This method currently returns a [String].
    /// This may change in the future.
    pub async fn get_metadata(&self) -> PmtResult<String> {
        let offset = self.header.metadata_offset as _;
        let length = self.header.metadata_length as _;
        let metadata = self.backend.read_exact(offset, length).await?;

        let decompressed_metadata =
            Self::decompress(self.header.internal_compression, metadata).await?;

        Ok(String::from_utf8(decompressed_metadata.to_vec())?)
    }

    #[cfg(feature = "tilejson")]
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
    async fn find_tile_entry(&self, tile_id: u64) -> Option<DirEntry> {
        let entry = self.root_directory.find_tile_id(tile_id);
        if let Some(entry) = entry {
            if entry.is_leaf() {
                return self.find_entry_rec(tile_id, entry, 0).await;
            }
        }
        entry.cloned()
    }

    #[async_recursion]
    async fn find_entry_rec(&self, tile_id: u64, entry: &DirEntry, depth: u8) -> Option<DirEntry> {
        // the recursion is done as two functions because it is a bit cleaner,
        // and it allows directory to be cached later without cloning it first.
        let offset = (self.header.leaf_offset + entry.offset) as _;

        let entry = match self.cache.get_dir_entry(offset, tile_id).await {
            DirCacheResult::NotCached => {
                // Cache miss - read from backend
                let length = entry.length as _;
                let dir = self.read_directory(offset, length).await.ok()?;
                let entry = dir.find_tile_id(tile_id).cloned();
                self.cache.insert_dir(offset, dir).await;
                entry
            }
            DirCacheResult::NotFound => None,
            DirCacheResult::Found(entry) => Some(entry),
        };

        if let Some(ref entry) = entry {
            if entry.is_leaf() {
                return if depth <= 4 {
                    self.find_entry_rec(tile_id, entry, depth + 1).await
                } else {
                    None
                };
            }
        }

        entry
    }

    async fn read_directory(&self, offset: usize, length: usize) -> PmtResult<Directory> {
        let data = self.backend.read_exact(offset, length).await?;
        Self::read_compressed_directory(self.header.internal_compression, data).await
    }

    async fn read_compressed_directory(
        compression: Compression,
        bytes: Bytes,
    ) -> PmtResult<Directory> {
        let decompressed_bytes = Self::decompress(compression, bytes).await?;
        Directory::try_from(decompressed_bytes)
    }

    async fn decompress(compression: Compression, bytes: Bytes) -> PmtResult<Bytes> {
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
impl AsyncPmTilesReader<HttpBackend, NoCache> {
    /// Creates a new `PMTiles` reader from a URL using the Reqwest backend.
    ///
    /// Fails if [url] does not exist or is an invalid archive. (Note: HTTP requests are made to validate it.)
    pub async fn new_with_url<U: IntoUrl>(client: Client, url: U) -> PmtResult<Self> {
        Self::new_with_cached_url(NoCache, client, url).await
    }
}

#[cfg(feature = "http-async")]
impl<C: DirectoryCache + Sync + Send> AsyncPmTilesReader<HttpBackend, C> {
    /// Creates a new `PMTiles` reader with cache from a URL using the Reqwest backend.
    ///
    /// Fails if [url] does not exist or is an invalid archive. (Note: HTTP requests are made to validate it.)
    pub async fn new_with_cached_url<U: IntoUrl>(
        cache: C,
        client: Client,
        url: U,
    ) -> PmtResult<Self> {
        let backend = HttpBackend::try_from(client, url)?;

        Self::try_from_cached_source(backend, cache).await
    }
}

#[cfg(feature = "mmap-async-tokio")]
impl AsyncPmTilesReader<MmapBackend, NoCache> {
    /// Creates a new `PMTiles` reader from a file path using the async mmap backend.
    ///
    /// Fails if [p] does not exist or is an invalid archive.
    pub async fn new_with_path<P: AsRef<Path>>(path: P) -> PmtResult<Self> {
        Self::new_with_cached_path(NoCache, path).await
    }
}

#[cfg(feature = "mmap-async-tokio")]
impl<C: DirectoryCache + Sync + Send> AsyncPmTilesReader<MmapBackend, C> {
    /// Creates a new cached `PMTiles` reader from a file path using the async mmap backend.
    ///
    /// Fails if [p] does not exist or is an invalid archive.
    pub async fn new_with_cached_path<P: AsRef<Path>>(cache: C, path: P) -> PmtResult<Self> {
        let backend = MmapBackend::try_from(path).await?;

        Self::try_from_cached_source(backend, cache).await
    }
}

#[cfg(any(feature = "s3-async-native", feature = "s3-async-rustls"))]
impl AsyncPmTilesReader<S3Backend, NoCache> {
    /// Creates a new `PMTiles` reader from a URL using the Reqwest backend.
    ///
    /// Fails if [url] does not exist or is an invalid archive. (Note: HTTP requests are made to validate it.)
    pub async fn new_with_bucket_path(bucket: s3::Bucket, path: String) -> PmtResult<Self> {
        Self::new_with_cached_bucket_path(NoCache, bucket, path).await
    }
}

#[cfg(any(feature = "s3-async-native", feature = "s3-async-rustls"))]
impl<C: DirectoryCache + Sync + Send> AsyncPmTilesReader<S3Backend, C> {
    /// Creates a new `PMTiles` reader with cache from a URL using the Reqwest backend.
    ///
    /// Fails if [url] does not exist or is an invalid archive. (Note: HTTP requests are made to validate it.)
    pub async fn new_with_cached_bucket_path(
        cache: C,
        bucket: s3::Bucket,
        path: String,
    ) -> PmtResult<Self> {
        let backend = S3Backend::from(bucket, path);

        Self::try_from_cached_source(backend, cache).await
    }
}

#[async_trait]
pub trait AsyncBackend {
    /// Reads exactly `length` bytes starting at `offset`
    async fn read_exact(&self, offset: usize, length: usize) -> PmtResult<Bytes>;

    /// Reads up to `length` bytes starting at `offset`.
    async fn read(&self, offset: usize, length: usize) -> PmtResult<Bytes>;
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
            tile.len(),
            fixture_bytes.len(),
            "Expected tile length to match."
        );
        assert_eq!(tile, fixture_bytes, "Expected tile to match fixture.");
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
