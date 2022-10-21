use std::fmt::{Debug, Formatter};

use async_recursion::async_recursion;
use async_trait::async_trait;
use hilbert_2d::Variant;
use tokio::io::AsyncReadExt;
use varint_rs::VarintReader;

use crate::error::Error;
use crate::header::{Compression, Header, TileType};

mod error;
mod header;
mod http;
mod mmap;

struct Metadata {}

struct Directory {
    entries: Vec<Entry>,
}

impl Debug for Directory {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!("Directory [entries: {}]", self.entries.len()))
    }
}

impl Directory {
    fn try_from(mut buffer: &[u8]) -> Result<Self, Error> {
        let n_entries = buffer.read_usize_varint()?;

        let mut entries = vec![Entry::default(); n_entries];

        // Read tile IDs
        let mut next_tile_id = 0;
        for entry in entries.iter_mut() {
            next_tile_id += buffer.read_u64_varint()?;
            entry.tile_id = next_tile_id;
        }

        // Read Run Lengths
        for entry in entries.iter_mut() {
            entry.run_length = buffer.read_u32_varint()?;
        }

        // Read Lengths
        for entry in entries.iter_mut() {
            entry.length = buffer.read_u32_varint()?;
        }

        // Read Offsets
        let mut last_entry: Option<&Entry> = None;
        for entry in entries.iter_mut() {
            let offset = buffer.read_u64_varint()?;
            entry.offset = if offset == 0 {
                let e = last_entry.ok_or(Error::InvalidEntry)?;
                e.offset + e.length as u64
            } else {
                offset - 1
            };
            last_entry = Some(entry);
        }

        Ok(Directory { entries })
    }

    fn find_tile_id(&self, tile_id: u64) -> Option<&Entry> {
        match self.entries.binary_search_by(|e| e.tile_id.cmp(&tile_id)) {
            Ok(idx) => self.entries.get(idx),
            Err(next_id) => {
                let previous_tile = self.entries.get(next_id - 1)?;

                if previous_tile.tile_id + previous_tile.run_length as u64 >= tile_id {
                    Some(previous_tile)
                } else {
                    None
                }
            }
        }
    }
}

#[derive(Clone, Default, Debug)]
struct Entry {
    tile_id: u64,
    offset: u64,
    length: u32,
    run_length: u32,
}

pub struct Tile {
    data: Vec<u8>,
    tile_type: TileType,
    tile_compression: Compression,
}

pub struct AsyncPmTiles<B: AsyncBackend> {
    header: Header,
    backend: B,
    root_directory: Directory,
}

impl<B: AsyncBackend + Sync + Send> AsyncPmTiles<B> {
    pub async fn try_from_source(backend: B) -> Result<Self, Error> {
        let mut header_bytes = [0; 127];
        backend.read_bytes(&mut header_bytes, 0).await?;
        let header = Header::try_from_bytes(&header_bytes)?;

        let root_directory = Self::read_directory_with_backend(
            &backend,
            header.root_offset as usize,
            header.root_length as usize,
        )
        .await?;

        Ok(Self {
            header,
            backend,
            root_directory,
        })
    }

    fn tile_id(z: u8, x: u64, y: u64) -> u64 {
        if z == 0 {
            return 0;
        }

        let base_id: u64 = 1 + (1..z).map(|i| 4u64.pow(i as u32)).sum::<u64>();

        let tile_id =
            hilbert_2d::xy2h_discrete(x as usize, y as usize, z as usize, Variant::Hilbert) as u64;

        base_id + tile_id
    }

    #[async_recursion]
    async fn find_tile_entry(
        &self,
        tile_id: u64,
        next_dir: Option<Directory>,
        depth: u8,
    ) -> Option<Entry> {
        // Max recursion...
        if !(depth < 4) {
            return None;
        }

        let next_dir = next_dir.as_ref().unwrap_or(&self.root_directory);

        match next_dir.find_tile_id(tile_id) {
            None => return None,
            Some(needle) => {
                if needle.run_length == 0 {
                    // Leaf directory
                    let next_dir = if let Some(next_dir) = self
                        .read_directory(
                            (self.header.leaf_offset + needle.offset) as usize,
                            needle.length as usize,
                        )
                        .await
                        .ok()
                    {
                        next_dir
                    } else {
                        return None;
                    };
                    self.find_tile_entry(tile_id, Some(next_dir), depth + 1)
                        .await
                } else {
                    return Some(needle.clone());
                }
            }
        }
    }

    pub async fn get_tile(&self, z: u8, x: u64, y: u64) -> Option<Tile> {
        let tile_id = Self::tile_id(z, x, y);
        let entry = self.find_tile_entry(tile_id, None, 0).await?;

        let mut data = vec![0; entry.length as usize];
        self.backend
            .read_bytes(
                data.as_mut_slice(),
                (self.header.data_offset + entry.offset) as usize,
            )
            .await
            .ok()?;

        Some(Tile {
            data,
            tile_type: self.header.tile_type,
            tile_compression: self.header.tile_compression,
        })
    }

    async fn read_directory(&self, offset: usize, length: usize) -> Result<Directory, Error> {
        Self::read_directory_with_backend(&self.backend, offset, length).await
    }

    async fn read_directory_with_backend(
        backend: &B,
        offset: usize,
        length: usize,
    ) -> Result<Directory, Error> {
        let mut directory_bytes = vec![0u8; length];
        backend
            .read_bytes(directory_bytes.as_mut_slice(), offset)
            .await?;

        let mut decompressed_bytes = Vec::with_capacity(length * 2);
        async_compression::tokio::bufread::GzipDecoder::new(directory_bytes.as_slice())
            .read_to_end(&mut decompressed_bytes)
            .await?;

        Directory::try_from(decompressed_bytes.as_slice())
    }
}

#[async_trait]
pub trait AsyncBackend {
    async fn read_bytes(&self, dst: &mut [u8], offset: usize) -> Result<(), Error>;

    async fn read_header_bytes(&self) -> Result<[u8; 127], Error> {
        let mut header_bytes = [0; 127];
        self.read_bytes(&mut header_bytes, 0).await?;

        Ok(header_bytes)
    }
}

#[cfg(test)]
mod test {
    use std::path::Path;

    use crate::mmap::MmapBackend;
    use crate::{AsyncBackend, Header};

    use super::AsyncPmTiles;

    #[test]
    fn test_tile_id() {
        assert_eq!(AsyncPmTiles::<MmapBackend>::tile_id(0, 0, 0), 0);
        assert_eq!(AsyncPmTiles::<MmapBackend>::tile_id(1, 1, 0), 4);
        assert_eq!(AsyncPmTiles::<MmapBackend>::tile_id(2, 1, 3), 11);
        assert_eq!(AsyncPmTiles::<MmapBackend>::tile_id(3, 3, 0), 26);
    }

    async fn create_backend() -> MmapBackend {
        MmapBackend::try_from_path(Path::new("fixtures/stamen_toner_z3.pmtiles"))
            .await
            .expect("Unable to open test file.")
    }

    #[tokio::test]
    async fn open_sanity_check() {
        let backend = create_backend().await;
        AsyncPmTiles::try_from_source(backend)
            .await
            .expect("Unable to open PMTiles");
    }

    #[tokio::test]
    async fn read_root_directory() {
        let backend = create_backend().await;
        let header = Header::try_from_bytes(
            &backend
                .read_header_bytes()
                .await
                .expect("Unable to read header bytes"),
        )
        .expect("Unable to parse header.");

        let directory = AsyncPmTiles::read_directory_with_backend(
            &backend,
            header.root_offset as usize,
            header.root_length as usize,
        )
        .await
        .expect("Unable to read directory");

        assert_eq!(directory.entries.len(), 84);
        // Note: this is not true for all tiles, just the first few...
        for nth in 0..10 {
            assert_eq!(directory.entries[nth].tile_id, nth as u64);
        }

        // ...it breaks pattern on the 59th tile
        assert_eq!(directory.entries[58].tile_id, 58);
        assert_eq!(directory.entries[58].run_length, 2);
        assert_eq!(directory.entries[58].offset, 422070);
        assert_eq!(directory.entries[58].length, 850);
    }

    async fn compare_tiles(z: u8, x: u64, y: u64, fixture_bytes: &[u8]) {
        let backend = create_backend().await;
        let tiles = AsyncPmTiles::try_from_source(backend)
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
    async fn test_leaf_directories() {
        let backend = MmapBackend::try_from_path(Path::new("fixtures/test.pmtiles"))
            .await
            .expect("Unable to open test file.");
        let tiles = AsyncPmTiles::try_from_source(backend)
            .await
            .expect("Unable to open PMTiles");

        let tile = tiles.get_tile(6, 31, 23).await;
        assert!(tile.is_some());
    }
}
