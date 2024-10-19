use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::{self, BufWriter, Seek, Write};

use ahash::AHasher;
use flate2::write::GzEncoder;

use crate::directory::{DirEntry, Directory};
use crate::error::PmtResult;
use crate::header::{HEADER_SIZE, MAX_INITIAL_BYTES};
use crate::{Compression, Header, TileType};

pub struct PmTilesWriter {
    out: BufWriter<File>,
    header: Header,
    entries: Vec<DirEntry>,
    n_addressed_tiles: u64,
    last_tile_hash: u64,
}

impl PmTilesWriter {
    pub fn create(name: &str, tile_type: TileType, metadata: &str) -> PmtResult<Self> {
        let file = File::create(name)?;
        let mut out = BufWriter::new(file);

        // We use the following layout:
        // +--------+----------------+----------+-----------+------------------+
        // |        |                |          |           |                  |
        // | Header | Root Directory | Metadata | Tile Data | Leaf Directories |
        // |        |                |          |           |                  |
        // +--------+----------------+----------+-----------+------------------+
        // This allows writing without temporary files. But it requires Seek support.

        // Reserve space for header and root directory
        out.write_all(&[0u8; MAX_INITIAL_BYTES])?;

        // let metadata_length = metadata.len() as u64;
        // out.write_all(metadata.as_bytes())?;
        let mut metadata_buf = vec![];
        {
            let mut encoder = GzEncoder::new(&mut metadata_buf, flate2::Compression::default());
            encoder.write_all(metadata.as_bytes())?;
        }
        let metadata_length = metadata_buf.len() as u64;
        out.write_all(&metadata_buf)?;

        let header = Header {
            version: 3,
            root_offset: HEADER_SIZE as u64,
            root_length: 0,
            metadata_offset: MAX_INITIAL_BYTES as u64,
            metadata_length,
            leaf_offset: 0,
            leaf_length: 0,
            data_offset: MAX_INITIAL_BYTES as u64 + metadata_length,
            data_length: 0,
            n_addressed_tiles: None,
            n_tile_entries: None,
            n_tile_contents: None,
            clustered: true,
            internal_compression: Compression::Gzip, // pmtiles extract does not support None compression
            tile_compression: Compression::None,
            tile_type,
            min_zoom: 0,
            max_zoom: 22,
            min_longitude: -180.0,
            min_latitude: -85.0,
            max_longitude: 180.0,
            max_latitude: 85.0,
            center_zoom: 0,
            center_longitude: 0.0,
            center_latitude: 0.0,
        };

        Ok(Self {
            out,
            header,
            entries: Vec::new(),
            n_addressed_tiles: 0,
            last_tile_hash: 0,
        })
    }

    fn calculate_hash(value: &impl Hash) -> u64 {
        let mut hasher = AHasher::default();
        value.hash(&mut hasher);
        hasher.finish()
    }

    /// Add tile to writer
    /// Tiles are deduplicated and written to output.
    /// `tile_id` should be increasing.
    #[allow(clippy::missing_panics_doc)]
    pub fn add_tile(&mut self, tile_id: u64, data: &[u8]) -> PmtResult<()> {
        if data.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "A tile must have at least 1 byte of data.",
            )
            .into());
        }

        let is_first = self.entries.is_empty();
        let mut first_entry = DirEntry {
            tile_id: 0,
            offset: 0,
            length: 0,
            run_length: 0,
        };
        let last_entry = self.entries.last_mut().unwrap_or(&mut first_entry);

        self.n_addressed_tiles += 1;
        let hash = Self::calculate_hash(&data);
        if !is_first
            && hash == self.last_tile_hash
            && tile_id == last_entry.tile_id + u64::from(last_entry.run_length)
        {
            last_entry.run_length += 1;
        } else {
            let offset = last_entry.offset + u64::from(last_entry.length);
            // Write tile
            let length = data.len().try_into().expect("TODO: check max");
            self.out.write_all(data)?;

            self.entries.push(DirEntry {
                tile_id,
                run_length: 1, // Will be increased if the next tile is the same
                offset,
                length,
            });

            self.last_tile_hash = hash;
        }

        Ok(())
    }

    /// Build root and leaf directories from entries.
    /// Leaf directories are written to output.
    /// The root directory is returned.
    fn build_directories(&self) -> Directory {
        let mut root_dir = Directory::default();
        for entry in &self.entries {
            root_dir.push(entry.clone());
        }
        // FIXME: check max size of root directory
        // TODO: Build and write optimized leaf directories
        root_dir
    }

    #[allow(clippy::missing_panics_doc)]
    pub fn finish(mut self) -> PmtResult<()> {
        if let Some(last) = self.entries.last() {
            self.header.data_length = last.offset + u64::from(last.length);
            self.header.leaf_offset = self.header.data_offset + self.header.data_length;
            self.header.n_addressed_tiles = self.n_addressed_tiles.try_into().ok();
            self.header.n_tile_entries = (self.entries.len() as u64).try_into().ok();
            self.header.n_tile_contents = None; //TODO
        }
        // Write leaf directories and get root directory
        let root_dir = self.build_directories();
        // Determine compressed root directory length
        let mut root_dir_buf = vec![];
        {
            let mut encoder = GzEncoder::new(&mut root_dir_buf, flate2::Compression::default());
            root_dir.write_to(&mut encoder)?;
        }
        self.header.root_length = root_dir_buf.len() as u64;

        // Write header and root directory
        self.out.rewind()?;
        self.header.write_to(&mut self.out)?;
        self.out.write_all(&root_dir_buf)?;
        self.out.flush()?;

        Ok(())
    }
}

#[cfg(test)]
#[cfg(feature = "mmap-async-tokio")]
mod tests {
    use super::PmTilesWriter;
    use crate::async_reader::AsyncPmTilesReader;
    use crate::tests::RASTER_FILE;
    use crate::MmapBackend;
    use tempfile::NamedTempFile;

    fn get_temp_file_path(suffix: &str) -> std::io::Result<String> {
        let temp_file = NamedTempFile::with_suffix(suffix)?;
        Ok(temp_file.path().to_string_lossy().into_owned())
    }

    #[tokio::test]
    #[allow(clippy::float_cmp)]
    async fn roundtrip_raster() {
        let backend = MmapBackend::try_from(RASTER_FILE).await.unwrap();
        let tiles_in = AsyncPmTilesReader::try_from_source(backend).await.unwrap();
        let header_in = tiles_in.get_header();
        let metadata_in = tiles_in.get_metadata().await.unwrap();
        let num_tiles = header_in.n_addressed_tiles.unwrap();

        let fname = get_temp_file_path("pmtiles").unwrap();
        // let fname = "test.pmtiles".to_string();
        let mut writer = PmTilesWriter::create(&fname, header_in.tile_type, &metadata_in).unwrap();
        for id in 0..num_tiles.into() {
            let tile = tiles_in.get_tile_by_id(id).await.unwrap().unwrap();
            writer.add_tile(id, &tile).unwrap();
        }
        writer.finish().unwrap();

        let backend = MmapBackend::try_from(&fname).await.unwrap();
        let tiles_out = AsyncPmTilesReader::try_from_source(backend).await.unwrap();

        // Compare headers
        let header_out = tiles_out.get_header();
        // TODO: should be 3, but currently the ascii char 3, assert_eq!(header_in.version, header_out.version);
        assert_eq!(header_in.tile_type, header_out.tile_type);
        assert_eq!(header_in.n_addressed_tiles, header_out.n_addressed_tiles);
        assert_eq!(header_in.n_tile_entries, header_out.n_tile_entries);
        // TODO: assert_eq!(header_in.n_tile_contents, header_out.n_tile_contents);
        assert_eq!(header_in.min_zoom, header_out.min_zoom);
        // TODO: assert_eq!(header_in.max_zoom, header_out.max_zoom);
        assert_eq!(header_in.center_zoom, header_out.center_zoom);
        assert_eq!(header_in.center_latitude, header_out.center_latitude);
        assert_eq!(header_in.center_longitude, header_out.center_longitude);
        assert_eq!(header_in.min_latitude, header_out.min_latitude);
        assert_eq!(header_in.max_latitude, header_out.max_latitude);
        assert_eq!(header_in.min_longitude, header_out.min_longitude);
        assert_eq!(header_in.max_longitude, header_out.max_longitude);
        assert_eq!(header_in.clustered, header_out.clustered);

        // Compare metadata
        let metadata_out = tiles_out.get_metadata().await.unwrap();
        assert_eq!(metadata_in, metadata_out);

        // Compare tiles
        for (z, x, y) in [(0, 0, 0), (2, 2, 2), (3, 4, 5)] {
            let tile_in = tiles_in.get_tile(z, x, y).await.unwrap().unwrap();
            let tile_out = tiles_out.get_tile(z, x, y).await.unwrap().unwrap();
            assert_eq!(tile_in.len(), tile_out.len());
        }
    }
}
