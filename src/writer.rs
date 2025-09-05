use std::io::{BufWriter, Seek, Write};

use countio::Counter;
use flate2::write::GzEncoder;

use crate::PmtError::UnsupportedCompression;
use crate::header::{HEADER_SIZE, MAX_INITIAL_BYTES};
use crate::{
    Compression, DirEntry, Directory, Header, PmtError, PmtResult, TileCoord, TileId, TileType,
};

const MAX_ROOT_DIR_BYTES: usize = MAX_INITIAL_BYTES - HEADER_SIZE;

/// Builder for creating a new writer.
pub struct PmTilesWriter {
    header: Header,
    metadata: String,
}

/// `PMTiles` streaming writer.
pub struct PmTilesStreamWriter<W: Write + Seek> {
    out: Counter<BufWriter<W>>,
    header: Header,
    entries: Vec<DirEntry>,
    n_addressed_tiles: u64,
    // TODO: Replace with digest HashMap for deduplicating non-subsequent tiles
    n_tile_contents: u64,
    // In our implementation, n_tile_entries is always equal to n_tile_contents, but we store it separately for when we support more complex deduplication with the TODO above.
    n_tile_entries: u64,
    prev_tile_data: Vec<u8>,
}

pub(crate) trait WriteTo {
    fn write_to<W: Write>(&self, writer: &mut W) -> std::io::Result<()>;

    fn write_compressed_to<W: Write>(
        &self,
        writer: &mut W,
        compression: Compression,
    ) -> PmtResult<()> {
        match compression {
            Compression::None => self.write_to(writer)?,
            Compression::Gzip => {
                let mut encoder = GzEncoder::new(writer, flate2::Compression::default());
                self.write_to(&mut encoder)?;
            }
            v => Err(UnsupportedCompression(v))?,
        }
        Ok(())
    }

    fn write_compressed_to_counted<W: Write>(
        &self,
        writer: &mut Counter<W>,
        compression: Compression,
    ) -> PmtResult<usize> {
        let pos = writer.writer_bytes();
        self.write_compressed_to(writer, compression)?;
        Ok(writer.writer_bytes() - pos)
    }

    fn compressed_size(&self, compression: Compression) -> PmtResult<usize> {
        let mut devnull = Counter::new(std::io::sink());
        self.write_compressed_to(&mut devnull, compression)?;
        Ok(devnull.writer_bytes())
    }
}

impl WriteTo for [u8] {
    fn write_to<W: Write>(&self, writer: &mut W) -> std::io::Result<()> {
        writer.write_all(self)
    }
}

impl PmTilesWriter {
    /// Create a new `PMTiles` writer with default values.
    #[must_use]
    pub fn new(tile_type: TileType) -> Self {
        let tile_compression = match tile_type {
            TileType::Mvt => Compression::Gzip,
            _ => Compression::None,
        };
        let header = Header::new(tile_compression, tile_type);
        Self {
            header,
            metadata: "{}".to_string(),
        }
    }

    /// Set the compression for metadata and directories.
    #[must_use]
    pub fn internal_compression(mut self, compression: Compression) -> Self {
        self.header.internal_compression = compression;
        self
    }

    /// Set the compression for tile data.
    #[must_use]
    pub fn tile_compression(mut self, compression: Compression) -> Self {
        self.header.tile_compression = compression;
        self
    }

    /// Set the minimum zoom level of the tiles
    #[must_use]
    pub fn min_zoom(mut self, level: u8) -> Self {
        self.header.min_zoom = level;
        self
    }

    /// Set the maximum zoom level of the tiles
    #[must_use]
    pub fn max_zoom(mut self, level: u8) -> Self {
        self.header.max_zoom = level;
        self
    }

    /// Set the bounds of the tiles
    #[must_use]
    pub fn bounds(mut self, min_lon: f32, min_lat: f32, max_lon: f32, max_lat: f32) -> Self {
        self.header.min_latitude = min_lat;
        self.header.min_longitude = min_lon;
        self.header.max_latitude = max_lat;
        self.header.max_longitude = max_lon;
        self
    }

    /// Set the center zoom level.
    #[must_use]
    pub fn center_zoom(mut self, level: u8) -> Self {
        self.header.center_zoom = level;
        self
    }

    /// Set the center position.
    #[must_use]
    pub fn center(mut self, lon: f32, lat: f32) -> Self {
        self.header.center_latitude = lat;
        self.header.center_longitude = lon;
        self
    }

    /// Set the metadata, which must contain a valid JSON object.
    ///
    /// If the tile type has a value of MVT Vector Tile, the object must contain a key of `vector_layers` as described in the `TileJSON` 3.0 specification.
    #[must_use]
    pub fn metadata(mut self, metadata: &str) -> Self {
        self.metadata = metadata.to_string();
        self
    }

    /// Create a new `PMTiles` writer.
    pub fn create<W: Write + Seek>(self, writer: W) -> PmtResult<PmTilesStreamWriter<W>> {
        let mut out = Counter::new(BufWriter::new(writer));

        // We use the following layout:
        // +--------+----------------+----------+-----------+------------------+
        // |        |                |          |           |                  |
        // | Header | Root Directory | Metadata | Tile Data | Leaf Directories |
        // |        |                |          |           |                  |
        // +--------+----------------+----------+-----------+------------------+
        // This allows writing without temporary files. But it requires Seek support.

        // Reserve space for the header and root directory
        out.write_all(&[0u8; MAX_INITIAL_BYTES])?;

        let metadata_length = self
            .metadata
            .as_bytes()
            .write_compressed_to_counted(&mut out, self.header.internal_compression)?
            as u64;

        let mut writer = PmTilesStreamWriter {
            out,
            header: self.header,
            entries: Vec::new(),
            n_addressed_tiles: 0,
            n_tile_contents: 0,
            n_tile_entries: 0,
            prev_tile_data: vec![],
        };
        writer.header.metadata_length = metadata_length;
        writer.header.data_offset = MAX_INITIAL_BYTES as u64 + metadata_length;

        Ok(writer)
    }
}

impl<W: Write + Seek> PmTilesStreamWriter<W> {
    /// Add a tile to the writer.
    ///
    /// Tiles are deduplicated and written to output.
    /// The `tile_id` generated from `z/x/y` should be increasing for best read performance.
    pub fn add_tile(&mut self, coord: TileCoord, data: &[u8]) -> PmtResult<()> {
        self.add_tile_by_id(coord.into(), data)
    }

    /// Add a tile to the writer.
    ///
    /// Tiles are deduplicated and written to output.
    /// The `tile_id` should be increasing for best read performance.
    fn add_tile_by_id(&mut self, tile_id: TileId, data: &[u8]) -> PmtResult<()> {
        if data.is_empty() {
            // Ignore empty tiles, since the spec does not allow storing them
            return Ok(());
        }

        let tile_id = tile_id.value();
        let is_first = self.entries.is_empty();
        if is_first && tile_id > 0 {
            self.header.clustered = false;
        }
        let mut first_entry = DirEntry {
            tile_id: 0,
            offset: 0,
            length: 0,
            run_length: 0,
        };
        let last_entry = self.entries.last_mut().unwrap_or(&mut first_entry);

        self.n_addressed_tiles += 1;
        if !is_first
            && self.prev_tile_data == data
            && tile_id == last_entry.tile_id + u64::from(last_entry.run_length)
        {
            last_entry.run_length += 1;
        } else {
            let offset = last_entry.offset + u64::from(last_entry.length);
            // Write tile
            let len =
                data.write_compressed_to_counted(&mut self.out, self.header.tile_compression)?;
            let length = into_u32(len)?;
            self.n_tile_contents += 1;
            if tile_id != last_entry.tile_id + u64::from(last_entry.run_length) {
                self.header.clustered = false;
            }

            self.n_tile_entries += 1;
            self.entries.push(DirEntry {
                tile_id,
                run_length: 1, // Will be increased by following identical tiles
                offset,
                length,
            });

            self.prev_tile_data = data.to_vec();
        }

        Ok(())
    }

    /// Build root and leaf directories from entries.
    /// Leaf directories are written to the output.
    /// The root directory is returned.
    /// The entries are consumed.
    /// The leaf directory metadata is written to the header.
    fn build_directories(&mut self) -> PmtResult<Directory> {
        if !self.header.clustered {
            // Spec does only say that leaf directories *should* be in ascending order,
            // but sorted directories are better for readers anyway.
            self.entries.sort_by_key(|entry| entry.tile_id);
        }
        let (root_dir, leaf_dirs) = self.optimize_directories(MAX_ROOT_DIR_BYTES)?;
        let mut leaves_bytes = 0usize;

        // If we have leaf directories, record their starting offset before writing them.
        if !leaf_dirs.is_empty() {
            self.header.leaf_offset = self.out.writer_bytes() as u64;
        }

        for leaf in &leaf_dirs {
            let leaf_bytes =
                leaf.write_compressed_to_counted(&mut self.out, self.header.internal_compression)?;
            leaves_bytes += leaf_bytes;
        }

        self.header.leaf_length = leaves_bytes as u64;
        Ok(root_dir)
    }

    fn optimize_directories(
        &mut self,
        target_root_len: usize,
    ) -> PmtResult<(Directory, Vec<Directory>)> {
        // Same logic as go-pmtiles (https://github.com/protomaps/go-pmtiles/blob/f1c24e6/pmtiles/directory.go#L368-L396)
        // and planetiler (https://github.com/onthegomap/planetiler/blob/6b3e152/planetiler-core/src/main/java/com/onthegomap/planetiler/pmtiles/WriteablePmtiles.java#L96-L118)

        // Case 1: let's see if the root directory fits without leaves
        if self.entries.len() < 16_384 {
            // we don't need self.entries anymore, so we'll put it in the root_dir directly
            let root_dir = Directory::from_entries(std::mem::take(&mut self.entries));
            let root_bytes = root_dir.compressed_size(self.header.internal_compression)?;
            if root_bytes <= target_root_len {
                return Ok((root_dir, vec![]));
            }
            // it didn't fit - go to the next case; put the entries back
            self.entries = root_dir.entries;
        }

        // TODO: case 2: mixed tile entries/directory entries in root

        // case 3: root directory is leaf pointers only
        // use an iterative method, increasing the size of the leaf directory until the root fits

        let mut leaf_size = (self.entries.len() / 3500).max(4096);
        loop {
            let (root_dir, leaf_dirs) = self.build_roots_leaves(leaf_size)?;
            let root_bytes = root_dir.compressed_size(self.header.internal_compression)?;
            if root_bytes <= target_root_len {
                return Ok((root_dir, leaf_dirs));
            }
            leaf_size += leaf_size / 5; // go-pmtiles: leaf_size *= 1.2
        }
    }

    /// Build root directory and leaf directories from entries, given a leaf size.
    /// The leaf directories are not written to output.
    /// The root directory is returned.
    fn build_roots_leaves(&self, leaf_size: usize) -> PmtResult<(Directory, Vec<Directory>)> {
        let mut root_dir = Directory::with_capacity(self.entries.len() / leaf_size);
        let mut leaves = Vec::with_capacity(self.entries.len() / leaf_size);
        let mut offset = 0;
        for chunk in self.entries.chunks(leaf_size) {
            let leaf = Directory::from_entries(chunk.to_vec());
            let leaf_size = leaf.compressed_size(self.header.internal_compression)?;
            leaves.push(leaf);

            root_dir.push(DirEntry {
                tile_id: chunk[0].tile_id,
                offset,
                length: into_u32(leaf_size)?,
                run_length: 0,
            });
            offset += leaf_size as u64;
        }

        Ok((root_dir, leaves))
    }

    /// Finish writing the `PMTiles` file.
    pub fn finalize(mut self) -> PmtResult<()> {
        // We're done writing data, so we can set the data_length here.
        self.header.data_length =
            self.out.writer_bytes() as u64 - MAX_INITIAL_BYTES as u64 - self.header.metadata_length;

        // Write leaf directories and get a root directory
        let root_dir = self.build_directories()?;

        self.header.n_addressed_tiles = self.n_addressed_tiles.try_into().ok();
        self.header.n_tile_contents = self.n_tile_contents.try_into().ok();
        self.header.n_tile_entries = self.n_tile_entries.try_into().ok();

        // Determine compressed root directory length
        let mut root_dir_buf = vec![];
        root_dir.write_compressed_to(&mut root_dir_buf, self.header.internal_compression)?;
        self.header.root_length = root_dir_buf.len() as u64;

        // Write header and root directory
        self.out.rewind()?;
        self.header.write_to(&mut self.out)?;
        self.out.write_all(&root_dir_buf)?;
        self.out.flush()?;

        Ok(())
    }
}

fn into_u32(v: usize) -> PmtResult<u32> {
    v.try_into().map_err(|_| PmtError::IndexEntryOverflow)
}

#[cfg(test)]
#[cfg(feature = "mmap-async-tokio")]
#[expect(clippy::float_cmp)]
mod tests {
    use std::fs::File;
    use std::num::NonZeroU64;
    use std::sync::Arc;

    use futures_util::TryStreamExt;
    use tempfile::NamedTempFile;

    use crate::tests::RASTER_FILE;
    use crate::{
        AsyncPmTilesReader, Compression, MmapBackend, PmTilesWriter, TileCoord, TileId, TileType,
    };

    fn get_temp_file_path(suffix: &str) -> std::io::Result<String> {
        let temp_file = NamedTempFile::with_suffix(suffix)?;
        Ok(temp_file.path().to_string_lossy().into_owned())
    }

    #[tokio::test]
    async fn roundtrip_raster() {
        let backend = MmapBackend::try_from(RASTER_FILE).await.unwrap();
        let tiles_in = AsyncPmTilesReader::try_from_source(backend).await.unwrap();
        let header_in = tiles_in.get_header();
        let metadata_in = tiles_in.get_metadata().await.unwrap();
        let num_tiles = header_in.n_addressed_tiles.unwrap();

        let path = get_temp_file_path("pmtiles").unwrap();
        // let path = "test.pmtiles".to_string();
        let file = File::create(path.clone()).unwrap();
        let mut writer = PmTilesWriter::new(header_in.tile_type)
            .max_zoom(header_in.max_zoom)
            .metadata(&metadata_in)
            .create(file)
            .unwrap();
        for id in 0..num_tiles.into() {
            let id = TileId::new(id).unwrap();
            let tile = tiles_in.get_tile(id).await.unwrap().unwrap();
            writer.add_tile_by_id(id, &tile).unwrap();
        }
        writer.finalize().unwrap();

        let backend = MmapBackend::try_from(&path).await.unwrap();
        let tiles_out = AsyncPmTilesReader::try_from_source(backend).await.unwrap();

        // Compare headers
        let header_out = tiles_out.get_header();
        // TODO: should be 3, but currently the ascii char 3, assert_eq!(header_in.version, header_out.version);
        assert_eq!(header_in.tile_type, header_out.tile_type);
        assert_eq!(header_in.n_addressed_tiles, header_out.n_addressed_tiles);
        assert_eq!(header_in.n_tile_entries, header_out.n_tile_entries);
        // assert_eq!(header_in.n_tile_contents, header_out.n_tile_contents);
        assert_eq!(Some(84), header_out.n_tile_contents.map(Into::into));
        assert_eq!(header_in.min_zoom, header_out.min_zoom);
        assert_eq!(header_in.max_zoom, header_out.max_zoom);
        assert_eq!(header_in.center_zoom, header_out.center_zoom);
        assert_eq!(header_in.center_latitude, header_out.center_latitude);
        assert_eq!(header_in.center_longitude, header_out.center_longitude);
        assert_eq!(
            header_in.min_latitude.round(),
            header_out.min_latitude.round()
        );
        assert_eq!(
            header_in.max_latitude.round(),
            header_out.max_latitude.round()
        );
        assert_eq!(header_in.min_longitude, header_out.min_longitude);
        assert_eq!(header_in.max_longitude, header_out.max_longitude);
        assert_eq!(header_in.clustered, header_out.clustered);

        // Compare metadata
        let metadata_out = tiles_out.get_metadata().await.unwrap();
        assert_eq!(metadata_in, metadata_out);

        // Compare tiles
        for (z, x, y) in [(0, 0, 0), (2, 2, 2), (3, 4, 5)] {
            let coord = TileCoord::new(z, x, y).unwrap();
            let tile_in = tiles_in.get_tile(coord).await.unwrap().unwrap();
            let tile_out = tiles_out.get_tile(coord).await.unwrap().unwrap();
            assert_eq!(tile_in.len(), tile_out.len());
        }
    }

    fn gen_entries(num_tiles: u64) -> String {
        let path = get_temp_file_path("pmtiles").unwrap();
        let file = File::create(&path).unwrap();
        let mut writer = PmTilesWriter::new(TileType::Png)
            // flate2 compression is extremely slow in debug mode
            .internal_compression(Compression::None)
            .create(file)
            .unwrap();
        for tile_id in 0..num_tiles {
            let data: Vec<u8> = tile_id.to_le_bytes().to_vec();
            writer
                .add_tile(TileId::new(tile_id).unwrap().into(), &data)
                .unwrap();
        }
        writer.finalize().unwrap();

        path
    }

    async fn verify_entries(file_path: &str, num_tiles: u64) {
        let backend = MmapBackend::try_from(file_path).await.unwrap();
        let tiles_out = Arc::new(AsyncPmTilesReader::try_from_source(backend).await.unwrap());
        let header_out = tiles_out.get_header();
        assert_eq!(header_out.n_addressed_tiles, NonZeroU64::new(num_tiles));
        assert_eq!(header_out.n_tile_entries, NonZeroU64::new(num_tiles));
        assert_eq!(header_out.n_tile_contents, NonZeroU64::new(num_tiles));
        let entries = tiles_out
            .clone()
            .entries()
            .try_collect::<Vec<_>>()
            .await
            .unwrap();
        let coords = entries
            .iter()
            .flat_map(|e| e.iter_coords())
            .collect::<Vec<_>>();

        assert_eq!(coords.len(), usize::try_from(num_tiles).unwrap());
        for tile_id in &[coords.first().unwrap(), coords.last().unwrap()] {
            let data: Vec<u8> = tile_id.value().to_le_bytes().to_vec();
            let tile_out = tiles_out.get_tile(**tile_id).await.unwrap().unwrap();
            assert_eq!(tile_out, data);
        }
    }

    #[tokio::test]
    async fn no_leaves() {
        let path = gen_entries(100);
        verify_entries(&path, 100).await;
    }

    #[tokio::test]
    async fn with_leaves() {
        let path = gen_entries(20000);
        verify_entries(&path, 20000).await;
    }

    #[test]
    fn unclustered() {
        let file = get_temp_file_path("pmtiles").unwrap();
        let file = File::create(file).unwrap();
        let mut writer = PmTilesWriter::new(TileType::Png).create(file).unwrap();

        let id = TileId::new(0).unwrap();
        writer.add_tile_by_id(id, &[0, 1, 2, 3]).unwrap();
        assert!(writer.header.clustered);

        let id = TileId::new(2).unwrap();
        writer.add_tile_by_id(id, &[0, 1, 2, 3]).unwrap();
        assert!(!writer.header.clustered);

        writer.finalize().unwrap();
    }
}
