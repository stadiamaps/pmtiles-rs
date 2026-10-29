mod compressor;
mod dedup;

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::hash::{BuildHasherDefault, Hash};
use std::io::{self, BufWriter, Seek, SeekFrom, Write};
use std::marker::PhantomData;
use std::mem;

pub use compressor::Compressor;
pub(crate) use compressor::{GzipCompressor, NoCompression};
pub use dedup::{CallerKey, ContentHash, DedupMode};
use twox_hash::{XxHash3_64, XxHash3_128};

use crate::header::{HEADER_SIZE, MAX_INITIAL_BYTES};
use crate::{
    Compression, DirEntry, Directory, Header, PmtError, PmtResult, TileCoord, TileId, TileType,
};

/// Maximum size of the root directory in bytes.
const MAX_ROOT_DIR_BYTES: u64 = (MAX_INITIAL_BYTES - HEADER_SIZE) as u64;

/// Builder for creating a new writer.
///
/// `D` is how the writer finds identical tiles, see [`DedupMode`].
pub struct PmTilesWriter<D: DedupMode = ContentHash> {
    header: Header,
    metadata: String,
    tile_compressor: Box<dyn Compressor>,
    internal_compressor: Box<dyn Compressor>,
    dedup: PhantomData<D>,
}

#[derive(Clone, Copy)]
struct TileContentLocation {
    offset: u64,
    length: u32,
}

/// `PMTiles` streaming writer.
///
/// `D` is how the writer finds identical tiles, see [`DedupMode`]. It decides the signature of
/// `add_tile`: [`PmTilesStreamWriter::<W, ContentHash>::add_tile`] takes the tile,
/// [`PmTilesStreamWriter::<W, CallerKey<K>>::add_tile`] also takes the caller's key.
pub struct PmTilesStreamWriter<W: Write + Seek, D: DedupMode = ContentHash> {
    state: WriterState<W, D::Key>,
    tile_compressor: Box<dyn Compressor>,
    internal_compressor: Box<dyn Compressor>,
    /// Metadata known only after the tiles, written at the end instead of the builder's.
    late_metadata: Option<String>,
}

/// Separated from `PmTilesStreamWriter` so that internal methods can borrow
/// this state mutably while `finalize` simultaneously reads the compressors.
struct WriterState<W: Write + Seek, K> {
    out: CountingWriter<BufWriter<W>>,
    header: Header,
    entries: Vec<DirEntry>,

    /// The number of addressable tiles in this archive.
    n_addressed_tiles: u64,

    /// The number of tile entries (not including directory entries) in this archive.
    n_tile_entries: u64,

    /// Locations of the tile contents that may repeat. Tiles added without a key are not remembered.
    tile_content_map: HashMap<K, TileContentLocation, BuildHasherDefault<XxHash3_64>>,

    /// The number of distinct tile contents written.
    n_tile_contents: u64,

    prev_content: Option<K>,
    prev_written_tile_offset: u64,
}

/// Sized wrapper around `&mut dyn Write` so it can be passed to generic `W: Write` methods.
struct DynWriter<'a>(&'a mut dyn Write);

impl Write for DynWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

/// Counts the bytes written as `u64`, so positions past 4 GiB fit on 32-bit targets.
/// Seeking does not change the count.
pub(crate) struct CountingWriter<W> {
    inner: W,
    written: u64,
}

impl<W> CountingWriter<W> {
    fn new(inner: W) -> Self {
        Self { inner, written: 0 }
    }

    fn written(&self) -> u64 {
        self.written
    }
}

impl<W: Write> Write for CountingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.written += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl<W: Seek> Seek for CountingWriter<W> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.inner.seek(pos)
    }
}

pub(crate) trait WriteTo {
    fn write_to<W: Write>(&self, writer: &mut W) -> io::Result<()>;

    fn write_compressed_to<W: Write>(
        &self,
        writer: &mut W,
        compressor: &dyn Compressor,
    ) -> PmtResult<()> {
        compressor.compress(
            &mut |encoder| self.write_to(&mut DynWriter(encoder)),
            writer,
        )
    }

    fn write_compressed_to_counted<W: Write>(
        &self,
        writer: &mut CountingWriter<W>,
        compressor: &dyn Compressor,
    ) -> PmtResult<u64> {
        let pos = writer.written();
        self.write_compressed_to(writer, compressor)?;
        Ok(writer.written() - pos)
    }

    fn compressed_size(&self, compressor: &dyn Compressor) -> PmtResult<u64> {
        let mut devnull = CountingWriter::new(io::sink());
        self.write_compressed_to(&mut devnull, compressor)?;
        Ok(devnull.written())
    }
}

impl WriteTo for [u8] {
    fn write_to<W: Write>(&self, writer: &mut W) -> io::Result<()> {
        writer.write_all(self)
    }
}

impl PmTilesWriter {
    /// Create a new `PMTiles` writer with default values.
    ///
    /// Identical tiles are found by hashing their content, see [`ContentHash`].
    /// Use [`dedup_by_key`](Self::dedup_by_key) to provide the keys yourself instead.
    #[must_use]
    pub fn new(tile_type: TileType) -> Self {
        let tile_compressor: Box<dyn Compressor> = match tile_type {
            TileType::Mvt => Box::new(GzipCompressor::default()),
            _ => Box::new(NoCompression),
        };
        let internal_compressor: Box<dyn Compressor> = Box::new(GzipCompressor::default());
        let header = Header::new(tile_compressor.compression(), tile_type);
        Self {
            header,
            metadata: "{}".to_string(),
            tile_compressor,
            internal_compressor,
            dedup: PhantomData,
        }
    }
}

impl<D: DedupMode> PmTilesWriter<D> {
    /// Find identical tiles by keys the caller provides instead of hashing, see [`CallerKey`].
    ///
    /// The writer's `add_tile` then takes an `Option<K>`: tiles given the same key must be identical
    /// and are stored once, and tiles given `None` are stored as is and not remembered.
    #[must_use]
    pub fn dedup_by_key<K: Copy + Eq + Hash>(self) -> PmTilesWriter<CallerKey<K>> {
        PmTilesWriter {
            header: self.header,
            metadata: self.metadata,
            tile_compressor: self.tile_compressor,
            internal_compressor: self.internal_compressor,
            dedup: PhantomData,
        }
    }

    /// Store tiles as given, without compressing them, and record `compression` as the tile
    /// compression in the header.
    ///
    /// Use this when the tiles are already compressed, e.g. when copying them from another archive.
    /// It replaces the tile compression set by [`tile_compression`](Self::tile_compression) or
    /// [`tile_codec`](Self::tile_codec), so an archive holds either tiles compressed by the writer
    /// or tiles compressed by the caller, never both.
    #[must_use]
    pub fn precompressed(mut self, compression: Compression) -> Self {
        self.header.tile_compression = compression;
        self.tile_compressor = Box::new(NoCompression);
        self
    }

    /// Set the compression for metadata and directories using a default compressor.
    ///
    /// For custom compression parameters, use [`internal_codec`](Self::internal_codec) instead.
    #[must_use]
    pub fn internal_compression(mut self, compression: Compression) -> Self {
        self.header.internal_compression = compression;
        self.internal_compressor = compression.into();
        self
    }

    /// Set the compression for tile data using a default compressor.
    ///
    /// For custom compression parameters, use [`tile_codec`](Self::tile_codec) instead.
    #[must_use]
    pub fn tile_compression(mut self, compression: Compression) -> Self {
        self.header.tile_compression = compression;
        self.tile_compressor = compression.into();
        self
    }

    /// Set the tile compressor. Also sets the header's tile compression.
    #[must_use]
    pub fn tile_codec(mut self, compressor: impl Compressor + 'static) -> Self {
        self.header.tile_compression = compressor.compression();
        self.tile_compressor = Box::new(compressor);
        self
    }

    /// Set the internal (metadata/directory) compressor.
    #[must_use]
    pub fn internal_codec(mut self, compressor: impl Compressor + 'static) -> Self {
        self.header.internal_compression = compressor.compression();
        self.internal_compressor = Box::new(compressor);
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
    pub fn bounds(mut self, min_lon: f64, min_lat: f64, max_lon: f64, max_lat: f64) -> Self {
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
    pub fn center(mut self, lon: f64, lat: f64) -> Self {
        self.header.center_latitude = lat;
        self.header.center_longitude = lon;
        self
    }

    /// Set the metadata, which must contain a valid JSON object.
    ///
    /// If the tile type has a value of MVT Vector Tile, the object must contain a key of `vector_layers` as described in the `TileJSON` 3.0 specification.
    ///
    /// If the metadata is only known after all tiles are written,
    /// use [`PmTilesStreamWriter::set_metadata`] instead.
    #[must_use]
    pub fn metadata(mut self, metadata: impl Into<String>) -> Self {
        self.metadata = metadata.into();
        self
    }

    /// Create a new `PMTiles` writer.
    ///
    /// # Errors
    ///
    /// If writing to the output stream fails
    pub fn create<W: Write + Seek>(self, writer: W) -> PmtResult<PmTilesStreamWriter<W, D>> {
        let mut out = CountingWriter::new(BufWriter::new(writer));

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
            .write_compressed_to_counted(&mut out, &self.internal_compressor)?;

        let mut state = WriterState {
            out,
            header: self.header,
            entries: Vec::new(),
            n_addressed_tiles: 0,
            n_tile_entries: 0,
            tile_content_map: HashMap::default(),
            n_tile_contents: 0,
            prev_content: None,
            prev_written_tile_offset: 0,
        };
        state.header.metadata_length = metadata_length;
        state.header.data_offset = MAX_INITIAL_BYTES as u64 + metadata_length;

        let writer = PmTilesStreamWriter {
            state,
            tile_compressor: self.tile_compressor,
            internal_compressor: self.internal_compressor,
            late_metadata: None,
        };

        Ok(writer)
    }
}

impl<W: Write + Seek> PmTilesStreamWriter<W, ContentHash> {
    /// Add a tile to the writer.
    ///
    /// Tiles are deduplicated by a 128-bit hash of `data` and written to output.
    /// Empty tiles are skipped, because the spec does not allow zero-length tile entries.
    /// The `tile_id` generated from `z/x/y` should be increasing for best read performance.
    ///
    /// # Errors
    ///
    /// If writing to the output stream fails
    pub fn add_tile(&mut self, coord: TileCoord, data: &[u8]) -> PmtResult<()> {
        let key = XxHash3_128::oneshot(data);
        self.state
            .add_tile(coord.into(), data, Some(key), &self.tile_compressor)
    }
}

impl<W: Write + Seek, K: Copy + Eq + Hash> PmTilesStreamWriter<W, CallerKey<K>> {
    /// Add a tile whose duplicates the caller has already found.
    ///
    /// Nothing is hashed. Tiles given the same `key` must have identical bytes: they are stored
    /// once, and consecutive ones as a single run. A tile given `None` is stored as is and not
    /// remembered. The writer then keeps in memory only the contents the caller expects to repeat,
    /// which matters for archives with hundreds of millions of distinct tiles.
    ///
    /// Empty tiles are skipped, because the spec does not allow zero-length tile entries.
    /// The `tile_id` generated from `z/x/y` should be increasing for best read performance.
    ///
    /// # Errors
    ///
    /// If writing to the output stream fails
    pub fn add_tile(&mut self, coord: TileCoord, data: &[u8], key: Option<K>) -> PmtResult<()> {
        self.state
            .add_tile(coord.into(), data, key, &self.tile_compressor)
    }
}

impl<W: Write + Seek, D: DedupMode> PmTilesStreamWriter<W, D> {
    /// Replace the builder's metadata with one known only after the tiles, e.g. the layers and zooms a
    /// generator produced. It is written after the tiles, and the header points at it.
    /// Same requirements as [`PmTilesWriter::metadata`] apply.
    ///
    /// The builder's metadata is still written at the start of the archive but is no longer referenced,
    /// so leave it at its default (`{}`) when planning to call this.
    pub fn set_metadata(&mut self, metadata: impl Into<String>) {
        self.late_metadata = Some(metadata.into());
    }

    /// Set the zoom range in the header, as [`PmTilesWriter::min_zoom`] and [`PmTilesWriter::max_zoom`]
    /// do, once the tiles are known.
    pub fn set_zoom_range(&mut self, min_zoom: u8, max_zoom: u8) {
        self.state.header.min_zoom = min_zoom;
        self.state.header.max_zoom = max_zoom;
    }

    /// Set the bounds in the header, as [`PmTilesWriter::bounds`] does, once the tiles are known.
    pub fn set_bounds(&mut self, min_lon: f64, min_lat: f64, max_lon: f64, max_lat: f64) {
        self.state.header.min_longitude = min_lon;
        self.state.header.min_latitude = min_lat;
        self.state.header.max_longitude = max_lon;
        self.state.header.max_latitude = max_lat;
    }

    /// Set the center in the header, as [`PmTilesWriter::center`] and [`PmTilesWriter::center_zoom`]
    /// do, once the tiles are known.
    pub fn set_center(&mut self, lon: f64, lat: f64, zoom: u8) {
        self.state.header.center_longitude = lon;
        self.state.header.center_latitude = lat;
        self.state.header.center_zoom = zoom;
    }
}

impl<W: Write + Seek, K: Copy + Eq + Hash> WriterState<W, K> {
    /// Add a tile, deduplicated by `key` if given. Empty tiles are skipped.
    /// The `tile_id` should be increasing for best read performance.
    fn add_tile(
        &mut self,
        tile_id: TileId,
        data: &[u8],
        key: Option<K>,
        compressor: &dyn Compressor,
    ) -> PmtResult<()> {
        if data.is_empty() {
            // Ignore empty tiles, since the spec does not allow storing them
            return Ok(());
        }

        let tile_id = tile_id.value();
        self.n_addressed_tiles += 1;

        // If the tile is identical to the previous one and the tile_id is consecutive, increase run_length
        if let Some(last_entry) = self.entries.last_mut() {
            if key.is_some()
                && self.prev_content == key
                && tile_id == last_entry.tile_id + u64::from(last_entry.run_length)
            {
                last_entry.run_length += 1;
                return Ok(());
            }

            // If the tile_id is not in order, mark as unclustered
            if tile_id < last_entry.tile_id + u64::from(last_entry.run_length) {
                self.header.clustered = false;
            }
        }

        // Either reuse the location of identical content, or write the tile data to the archive
        let loc = match key.map(|key| self.tile_content_map.entry(key)) {
            Some(Entry::Occupied(e)) => *e.get(),
            slot => {
                let offset = self.prev_written_tile_offset;
                let len = data.write_compressed_to_counted(&mut self.out, compressor)?;
                self.prev_written_tile_offset += len;
                self.n_tile_contents += 1;
                let loc = TileContentLocation {
                    offset,
                    length: into_u32(len)?,
                };
                if let Some(Entry::Vacant(e)) = slot {
                    e.insert(loc);
                }
                loc
            }
        };

        self.prev_content = key;

        self.n_tile_entries += 1;
        self.entries.push(DirEntry {
            tile_id,
            run_length: 1, // Will be increased by following identical tiles
            offset: loc.offset,
            length: loc.length,
        });

        Ok(())
    }

    /// Build root and leaf directories from entries.
    /// Leaf directories are written to the output.
    /// The root directory is returned.
    /// The entries are consumed.
    /// The leaf directory metadata is written to the header.
    fn build_directories(&mut self, compressor: &dyn Compressor) -> PmtResult<Directory> {
        if !self.header.clustered {
            // Spec does only say that leaf directories *should* be in ascending order,
            // but sorted directories are better for readers anyway.
            self.entries.sort_by_key(|entry| entry.tile_id);
        }
        let (root_dir, leaf_dirs) = self.optimize_directories(MAX_ROOT_DIR_BYTES, compressor)?;
        let mut leaves_bytes = 0;

        // If we have leaf directories, record their starting offset before writing them.
        if !leaf_dirs.is_empty() {
            self.header.leaf_offset = self.out.written();
        }

        for leaf in &leaf_dirs {
            leaves_bytes += leaf.write_compressed_to_counted(&mut self.out, compressor)?;
        }

        self.header.leaf_length = leaves_bytes;
        Ok(root_dir)
    }

    fn optimize_directories(
        &mut self,
        target_root_len: u64,
        compressor: &dyn Compressor,
    ) -> PmtResult<(Directory, Vec<Directory>)> {
        // Same logic as go-pmtiles (https://github.com/protomaps/go-pmtiles/blob/f1c24e6/pmtiles/directory.go#L368-L396)
        // and planetiler (https://github.com/onthegomap/planetiler/blob/6b3e152/planetiler-core/src/main/java/com/onthegomap/planetiler/pmtiles/WriteablePmtiles.java#L96-L118)

        // Case 1: let's see if the root directory fits without leaves
        if self.entries.len() < 16_384 {
            // we don't need self.entries anymore, so we'll put it in the root_dir directly
            let root_dir = Directory::from_entries(mem::take(&mut self.entries));
            let root_bytes = root_dir.compressed_size(compressor)?;
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
            let (root_dir, leaf_dirs) = self.build_roots_leaves(leaf_size, compressor)?;
            let root_bytes = root_dir.compressed_size(compressor)?;
            if root_bytes <= target_root_len {
                return Ok((root_dir, leaf_dirs));
            }
            leaf_size += leaf_size / 5; // go-pmtiles: leaf_size *= 1.2
        }
    }

    /// Build root directory and leaf directories from entries, given a leaf size.
    /// The leaf directories are not written to output.
    /// The root directory is returned.
    fn build_roots_leaves(
        &self,
        leaf_size: usize,
        compressor: &dyn Compressor,
    ) -> PmtResult<(Directory, Vec<Directory>)> {
        let mut root_dir = Directory::with_capacity(self.entries.len() / leaf_size);
        let mut leaves = Vec::with_capacity(self.entries.len() / leaf_size);
        let mut offset = 0;
        for chunk in self.entries.chunks(leaf_size) {
            let leaf = Directory::from_entries(chunk.to_vec());
            let leaf_size = leaf.compressed_size(compressor)?;
            leaves.push(leaf);

            root_dir.push(DirEntry {
                tile_id: chunk[0].tile_id,
                offset,
                length: into_u32(leaf_size)?,
                run_length: 0,
            });
            offset += leaf_size;
        }

        Ok((root_dir, leaves))
    }
}

impl<W: Write + Seek, D: DedupMode> PmTilesStreamWriter<W, D> {
    /// Finish writing the `PMTiles` file.
    ///
    /// # Errors
    ///
    /// This function will return an error if
    /// - writing to the output stream fails or
    /// - if the file structure is invalid.
    pub fn finalize(mut self) -> PmtResult<()> {
        let state = &mut self.state;

        // We're done writing data, so we can set the data_length here.
        state.header.data_length =
            state.out.written() - MAX_INITIAL_BYTES as u64 - state.header.metadata_length;

        // Write leaf directories and get a root directory
        let root_dir = state.build_directories(&self.internal_compressor)?;

        if let Some(metadata) = &self.late_metadata {
            state.header.metadata_offset = state.out.written();
            state.header.metadata_length = metadata
                .as_bytes()
                .write_compressed_to_counted(&mut state.out, &self.internal_compressor)?;
        }

        state.header.n_addressed_tiles = state.n_addressed_tiles.try_into().ok();
        state.header.n_tile_contents = state.n_tile_contents.try_into().ok();
        state.header.n_tile_entries = state.n_tile_entries.try_into().ok();

        // Determine compressed root directory length
        let mut root_dir_buf = vec![];
        root_dir.write_compressed_to(&mut root_dir_buf, &self.internal_compressor)?;
        state.header.root_length = root_dir_buf.len() as u64;

        // Write header and root directory
        state.out.rewind()?;
        state.header.write_to(&mut state.out)?;
        state.out.write_all(&root_dir_buf)?;
        state.out.flush()?;

        Ok(())
    }
}

fn into_u32(v: u64) -> PmtResult<u32> {
    v.try_into().map_err(|_| PmtError::IndexEntryOverflow)
}

#[cfg(test)]
#[cfg(feature = "mmap-async-tokio")]
#[allow(
    clippy::float_cmp,
    reason = "exact round-trips; clippy versions differ on flagging float `assert_eq!`"
)]
mod tests {
    use std::fs::File;
    use std::io;
    use std::num::NonZeroU64;
    use std::sync::Arc;

    use futures_util::TryStreamExt;
    use rstest::rstest;
    use tempfile::NamedTempFile;

    use crate::tests::RASTER_FILE;
    use crate::writer::GzipCompressor;
    use crate::{
        AsyncPmTilesReader, CallerKey, Compression, MmapBackend, PmTilesStreamWriter,
        PmTilesWriter, TileCoord, TileId, TileType,
    };

    fn get_temp_file_path(suffix: &str) -> io::Result<String> {
        let temp_file = NamedTempFile::with_suffix(suffix)?;
        Ok(temp_file.path().to_string_lossy().into_owned())
    }

    #[tokio::test]
    async fn builder_sets_header_fields() {
        let path = get_temp_file_path("pmtiles").unwrap();
        let mut writer = PmTilesWriter::new(TileType::Mvt)
            .min_zoom(1)
            .max_zoom(7)
            .bounds(-10.0, -20.0, 30.0, 40.0)
            .center_zoom(3)
            .center(1.0, 2.0)
            .create(File::create(&path).unwrap())
            .unwrap();
        writer
            .add_tile(TileCoord::new(1, 0, 0).unwrap(), b"tile")
            .unwrap();
        writer.finalize().unwrap();

        let backend = MmapBackend::try_from(&path).await.unwrap();
        let reader = AsyncPmTilesReader::try_from_source(backend).await.unwrap();
        let header = reader.get_header();
        assert_eq!(
            (header.min_zoom, header.max_zoom, header.center_zoom),
            (1, 7, 3)
        );
        assert_eq!(
            (
                header.min_longitude,
                header.min_latitude,
                header.max_longitude,
                header.max_latitude
            ),
            (-10.0, -20.0, 30.0, 40.0)
        );
        assert_eq!(
            (header.center_longitude, header.center_latitude),
            (1.0, 2.0)
        );
    }

    #[tokio::test]
    async fn metadata_and_zooms_set_after_the_tiles() {
        let path = get_temp_file_path("pmtiles").unwrap();
        let mut writer = PmTilesWriter::new(TileType::Mvt)
            .metadata(r#"{"name":"early"}"#)
            .tile_compression(Compression::None)
            .dedup_by_key::<u64>()
            .create(File::create(&path).unwrap())
            .unwrap();
        for id in 0..300 {
            let coord = TileId::new(id).unwrap().into();
            writer.add_tile(coord, &id.to_le_bytes(), None).unwrap();
        }
        writer.set_metadata(r#"{"name":"late"}"#);
        writer.set_zoom_range(1, 7);
        writer.set_bounds(-10.0, -20.0, 30.0, 40.0);
        writer.set_center(1.0, 2.0, 3);
        writer.finalize().unwrap();

        let backend = MmapBackend::try_from(&path).await.unwrap();
        let reader = AsyncPmTilesReader::try_from_source(backend).await.unwrap();
        assert_eq!(reader.get_metadata().await.unwrap(), r#"{"name":"late"}"#);
        let header = reader.get_header();
        assert_eq!(
            (header.min_zoom, header.max_zoom, header.center_zoom),
            (1, 7, 3)
        );
        assert_eq!((header.min_longitude, header.max_latitude), (-10.0, 40.0));
        assert_eq!(
            reader
                .get_tile(TileId::new(299).unwrap())
                .await
                .unwrap()
                .as_deref(),
            Some(&299u64.to_le_bytes()[..])
        );
    }

    #[tokio::test]
    async fn caller_dedup_keys_store_keyed_tiles_once() {
        let path = get_temp_file_path("pmtiles").unwrap();
        let mut writer = PmTilesWriter::new(TileType::Mvt)
            .tile_compression(Compression::None)
            .dedup_by_key::<u64>()
            .create(File::create(&path).unwrap())
            .unwrap();
        let tiles: [(&[u8], Option<u64>); 6] = [
            (b"root", None),
            (b"fill", Some(7)),
            (b"fill", Some(7)), // extends the previous run
            (b"other", None),
            (b"fill", Some(7)), // reuses the stored content in a new entry
            (b"other", None),   // unkeyed, so stored again
        ];
        for (id, (data, dedup)) in (0..).zip(tiles) {
            let coord = TileId::new(id).unwrap().into();
            writer.add_tile(coord, data, dedup).unwrap();
        }
        // empty tiles are skipped, keyed or not
        let empty_coord = TileId::new(6).unwrap().into();
        writer.add_tile(empty_coord, b"", Some(7)).unwrap();
        writer.add_tile(empty_coord, b"", None).unwrap();
        writer.finalize().unwrap();

        let backend = MmapBackend::try_from(&path).await.unwrap();
        let reader = AsyncPmTilesReader::try_from_source(backend).await.unwrap();
        let header = reader.get_header();
        assert_eq!(header.n_addressed_tiles, NonZeroU64::new(6));
        assert_eq!(header.n_tile_entries, NonZeroU64::new(5));
        assert_eq!(header.n_tile_contents, NonZeroU64::new(4));
        for (id, (data, _)) in (0..).zip(tiles) {
            let tile = reader.get_tile(TileId::new(id).unwrap()).await.unwrap();
            assert_eq!(tile.as_deref(), Some(data), "tile {id}");
        }
        let empty = reader.get_tile(TileId::new(6).unwrap()).await.unwrap();
        assert_eq!(empty, None);
    }

    #[tokio::test]
    async fn late_metadata_after_leaf_directories() {
        let path = get_temp_file_path("pmtiles").unwrap();
        let mut writer = PmTilesWriter::new(TileType::Png)
            // flate2 compression is extremely slow in debug mode
            .internal_compression(Compression::None)
            .dedup_by_key::<u64>()
            .create(File::create(&path).unwrap())
            .unwrap();
        for tile_id in 0..20_000_u64 {
            let coord = TileId::new(tile_id).unwrap().into();
            writer
                .add_tile(coord, &tile_id.to_le_bytes(), None)
                .unwrap();
        }
        writer.set_metadata(String::from(r#"{"name":"late"}"#));
        writer.finalize().unwrap();

        verify_entries(&path, 20_000).await;
        let backend = MmapBackend::try_from(&path).await.unwrap();
        let reader = AsyncPmTilesReader::try_from_source(backend).await.unwrap();
        let header = reader.get_header();
        assert!(header.leaf_length > 0, "test must produce leaf directories");
        assert_eq!(
            header.metadata_offset,
            header.leaf_offset + header.leaf_length
        );
        assert_eq!(reader.get_metadata().await.unwrap(), r#"{"name":"late"}"#);
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
            .metadata(metadata_in.as_str())
            .precompressed(header_in.tile_compression)
            .create(file)
            .unwrap();
        for id in 0..num_tiles.into() {
            let id = TileId::new(id).unwrap();
            let tile = tiles_in.get_tile(id).await.unwrap().unwrap();
            writer.add_tile(id.into(), &tile).unwrap();
        }
        writer.finalize().unwrap();

        let backend = MmapBackend::try_from(&path).await.unwrap();
        let tiles_out = AsyncPmTilesReader::try_from_source(backend).await.unwrap();

        // Compare headers
        let header_out = tiles_out.get_header();
        // TODO: should be 3, but currently the ascii char 3, assert_eq!(header_in.version, header_out.version);
        assert_eq!(header_in.tile_type, header_out.tile_type);
        assert_eq!(header_in.tile_compression, header_out.tile_compression);
        assert_eq!(header_in.n_addressed_tiles, header_out.n_addressed_tiles);
        assert_eq!(header_in.n_tile_entries, header_out.n_tile_entries);
        assert_eq!(header_in.n_tile_contents, header_out.n_tile_contents);
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
    fn counting_writer_passes_4_gib() {
        use std::io::Write as _;

        use super::CountingWriter;

        let mut out = CountingWriter {
            inner: io::sink(),
            written: u64::from(u32::MAX) - 1,
        };
        out.write_all(b"four").unwrap();
        assert_eq!(out.written(), u64::from(u32::MAX) + 3);
    }

    #[test]
    fn writer_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<PmTilesStreamWriter<File>>();
        assert_send::<PmTilesStreamWriter<File, CallerKey<u64>>>();
    }

    #[test]
    fn unclustered() {
        let file = get_temp_file_path("pmtiles").unwrap();
        let file = File::create(file).unwrap();
        let mut writer = PmTilesWriter::new(TileType::Png).create(file).unwrap();
        assert_eq!(writer.state.header.tile_compression, Compression::None);

        let id = TileId::new(2).unwrap();
        writer.add_tile(id.into(), &[0, 1, 2, 3]).unwrap();
        assert!(writer.state.header.clustered);

        let id = TileId::new(0).unwrap();
        writer.add_tile(id.into(), &[0, 1, 2, 3]).unwrap();
        assert!(!writer.state.header.clustered);

        writer.finalize().unwrap();
    }

    #[rstest]
    #[case(Compression::Gzip)]
    #[cfg_attr(feature = "brotli", case(Compression::Brotli))]
    #[cfg_attr(feature = "zstd", case(Compression::Zstd))]
    #[tokio::test]
    async fn precompressed_tiles(#[case] compression: Compression) {
        let path = get_temp_file_path("pmtiles").unwrap();
        let file = File::create(&path).unwrap();
        let mut writer = PmTilesWriter::new(TileType::Mvt)
            .precompressed(compression)
            .dedup_by_key::<u8>()
            .create(file)
            .unwrap();

        // the "compressed" bytes are stored as given, whatever the header says
        let coord = |id| TileId::new(id).unwrap().into();
        writer.add_tile(coord(0), &[0], None).unwrap();
        writer.add_tile(coord(1), &[1], Some(1)).unwrap();
        writer.add_tile(coord(2), &[1], Some(1)).unwrap();
        writer.finalize().unwrap();

        // Read it out
        let backend = MmapBackend::try_from(&path).await.unwrap();
        let tiles_out = AsyncPmTilesReader::try_from_source(backend).await.unwrap();

        let header = tiles_out.get_header();
        assert_eq!(header.tile_compression, compression);
        assert_eq!(header.n_addressed_tiles, NonZeroU64::new(3));
        assert_eq!(header.n_tile_entries, NonZeroU64::new(2));
        assert_eq!(header.n_tile_contents, NonZeroU64::new(2));
        for (id, data) in [(0, [0]), (1, [1]), (2, [1])] {
            let tile = tiles_out.get_tile(TileId::new(id).unwrap()).await.unwrap();
            assert_eq!(tile.as_deref(), Some(&data[..]), "tile {id}");
        }
    }

    /// Tests that internal compression (for directories/metadata) roundtrips correctly
    /// with all supported codecs.
    #[rstest]
    #[case(Compression::Gzip)]
    #[cfg_attr(feature = "brotli", case(Compression::Brotli))]
    #[cfg_attr(feature = "zstd", case(Compression::Zstd))]
    #[tokio::test]
    async fn internal_compression_roundtrip(#[case] compression: Compression) {
        let path = get_temp_file_path("pmtiles").unwrap();
        let file = File::create(&path).unwrap();

        let test_metadata = r#"{"name":"test","description":"internal compression test"}"#;

        let mut writer = PmTilesWriter::new(TileType::Mvt)
            .internal_compression(compression)
            .metadata(test_metadata)
            .create(file)
            .unwrap();

        // Add enough tiles to exercise directory compression
        for tile_id in 0..100u64 {
            let data: Vec<u8> = tile_id.to_le_bytes().to_vec();
            writer
                .add_tile(TileId::new(tile_id).unwrap().into(), &data)
                .unwrap();
        }
        writer.finalize().unwrap();

        // Read it back and verify internal compression was used
        let backend = MmapBackend::try_from(&path).await.unwrap();
        let tiles_out = Arc::new(AsyncPmTilesReader::try_from_source(backend).await.unwrap());

        let header = tiles_out.get_header();
        assert_eq!(header.internal_compression, compression);

        // Verify metadata can be decompressed and read
        let metadata_out = tiles_out.get_metadata().await.unwrap();
        assert_eq!(metadata_out, test_metadata);

        // Verify directory entries can be read (exercises directory decompression)
        let entries = tiles_out
            .clone()
            .entries()
            .try_collect::<Vec<_>>()
            .await
            .unwrap();
        assert_eq!(entries.len(), 100);

        // Verify tiles can be decompressed
        let tile_0 = tiles_out
            .get_tile_decompressed(TileId::new(0).unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(*tile_0, 0u64.to_le_bytes());

        let tile_99 = tiles_out
            .get_tile_decompressed(TileId::new(99).unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(*tile_99, 99u64.to_le_bytes());
    }

    #[tokio::test]
    async fn dedup_nonconsecutive_tiles_no_rle() {
        // Create archive with tiles A, B, C where A == C and B differs.
        let path = get_temp_file_path("pmtiles").unwrap();
        let file = File::create(&path).unwrap();
        let mut writer = PmTilesWriter::new(TileType::Png)
            .internal_compression(Compression::None)
            .create(file)
            .unwrap();

        // A == C, B differs.
        let a = b"ABC";
        let b = b"X";
        let c = b"ABC";

        writer.add_tile(TileId::new(0).unwrap().into(), a).unwrap();
        writer.add_tile(TileId::new(1).unwrap().into(), b).unwrap();
        writer.add_tile(TileId::new(2).unwrap().into(), c).unwrap();
        writer.finalize().unwrap();

        // Open and verify: 3 addressed/entries, 2 unique contents (A and C deduped), no RLE.
        let backend = MmapBackend::try_from(&path).await.unwrap();
        let tiles_out = Arc::new(AsyncPmTilesReader::try_from_source(backend).await.unwrap());
        let header = tiles_out.get_header();
        assert_eq!(header.n_addressed_tiles, NonZeroU64::new(3));
        assert_eq!(header.n_tile_entries, NonZeroU64::new(3));
        assert_eq!(header.n_tile_contents, NonZeroU64::new(2));

        let entries = tiles_out
            .clone()
            .entries()
            .try_collect::<Vec<_>>()
            .await
            .unwrap();
        assert_eq!(entries.len(), 3);

        let e0 = entries.iter().find(|e| e.tile_id == 0).unwrap();
        let e1 = entries.iter().find(|e| e.tile_id == 1).unwrap();
        let e2 = entries.iter().find(|e| e.tile_id == 2).unwrap();

        // No RLE should be used for non-consecutive identical tiles.
        assert_eq!(e0.run_length, 1);
        assert_eq!(e1.run_length, 1);
        assert_eq!(e2.run_length, 1);

        // A and C should refer to the same bytes in the archive.
        assert_eq!(e0.offset, e2.offset);
        assert_eq!(e0.length, e2.length);

        // B should point to different bytes.
        assert_ne!(e1.offset, e0.offset);
    }

    #[cfg(feature = "zstd")]
    #[tokio::test]
    async fn zstd_compressor_roundtrip() {
        use super::compressor::ZstdCompressor;

        let test_data = b"hello pmtiles zstd compressor with custom level";

        let path = get_temp_file_path("pmtiles").unwrap();
        let file = File::create(&path).unwrap();
        let mut writer = PmTilesWriter::new(TileType::Mvt)
            .tile_codec(ZstdCompressor(19))
            .create(file)
            .unwrap();

        let tile_id = TileId::new(0).unwrap();
        writer.add_tile(tile_id.into(), test_data).unwrap();
        writer.finalize().unwrap();

        let backend = MmapBackend::try_from(&path).await.unwrap();
        let tiles_out = AsyncPmTilesReader::try_from_source(backend).await.unwrap();
        assert_eq!(tiles_out.get_header().tile_compression, Compression::Zstd);

        let tile = tiles_out
            .get_tile_decompressed(tile_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&*tile, test_data);
    }

    #[tokio::test]
    async fn gzip_compressor_best_roundtrip() {
        let test_data = b"hello pmtiles gzip best compressor with enough data to see a difference";

        let path = get_temp_file_path("pmtiles").unwrap();
        let file = File::create(&path).unwrap();
        let mut writer = PmTilesWriter::new(TileType::Mvt)
            .tile_codec(GzipCompressor(flate2::Compression::best()))
            .create(file)
            .unwrap();

        let tile_id = TileId::new(0).unwrap();
        writer.add_tile(tile_id.into(), test_data).unwrap();
        writer.finalize().unwrap();

        let backend = MmapBackend::try_from(&path).await.unwrap();
        let tiles_out = AsyncPmTilesReader::try_from_source(backend).await.unwrap();
        assert_eq!(tiles_out.get_header().tile_compression, Compression::Gzip);

        let tile = tiles_out
            .get_tile_decompressed(tile_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&*tile, test_data);
    }

    #[tokio::test]
    async fn custom_compressor_roundtrip() {
        use std::io::Write;

        use super::{Compressor, NoCompression};
        use crate::PmtResult;

        /// A custom compressor that just delegates to `NoCompression`
        /// (for testing the trait mechanism).
        struct CustomTestCompressor;

        impl Compressor for CustomTestCompressor {
            fn compression(&self) -> Compression {
                // Use None so we can read back without decompression issues
                Compression::None
            }

            fn compress(
                &self,
                f: &mut dyn FnMut(&mut dyn Write) -> io::Result<()>,
                writer: &mut dyn Write,
            ) -> PmtResult<()> {
                NoCompression.compress(f, writer)
            }
        }

        let path = get_temp_file_path("pmtiles").unwrap();
        let file = File::create(&path).unwrap();
        let mut writer = PmTilesWriter::new(TileType::Png)
            .tile_codec(CustomTestCompressor)
            .create(file)
            .unwrap();

        let test_data = b"custom compressor test data";
        let tile_id = TileId::new(0).unwrap();
        writer.add_tile(tile_id.into(), test_data).unwrap();
        writer.finalize().unwrap();

        let backend = MmapBackend::try_from(&path).await.unwrap();
        let tiles_out = AsyncPmTilesReader::try_from_source(backend).await.unwrap();

        let tile = tiles_out.get_tile(tile_id).await.unwrap().unwrap();
        assert_eq!(&*tile, test_data);
    }

    #[cfg(feature = "brotli")]
    #[tokio::test]
    async fn brotli_compressor_roundtrip() {
        use brotli::enc::BrotliEncoderParams;

        use super::compressor::BrotliCompressor;

        let test_data = b"hello pmtiles brotli compressor with enough data to see a difference";

        let path = get_temp_file_path("pmtiles").unwrap();
        let file = File::create(&path).unwrap();
        let mut writer = PmTilesWriter::new(TileType::Mvt)
            .tile_codec(BrotliCompressor(BrotliEncoderParams {
                quality: 5,
                ..Default::default()
            }))
            .create(file)
            .unwrap();

        let tile_id = TileId::new(0).unwrap();
        writer.add_tile(tile_id.into(), test_data).unwrap();
        writer.finalize().unwrap();

        let backend = MmapBackend::try_from(&path).await.unwrap();
        let tiles_out = AsyncPmTilesReader::try_from_source(backend).await.unwrap();
        assert_eq!(tiles_out.get_header().tile_compression, Compression::Brotli);

        let tile = tiles_out
            .get_tile_decompressed(tile_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&*tile, test_data);
    }

    #[tokio::test]
    async fn internal_codec_roundtrip() {
        let test_metadata = r#"{"name":"test","description":"internal codec test"}"#;

        let path = get_temp_file_path("pmtiles").unwrap();
        let file = File::create(&path).unwrap();
        let mut writer = PmTilesWriter::new(TileType::Mvt)
            .internal_codec(GzipCompressor(flate2::Compression::best()))
            .metadata(test_metadata)
            .create(file)
            .unwrap();

        for tile_id in 0..100u64 {
            let data: Vec<u8> = tile_id.to_le_bytes().to_vec();
            writer
                .add_tile(TileId::new(tile_id).unwrap().into(), &data)
                .unwrap();
        }
        writer.finalize().unwrap();

        let backend = MmapBackend::try_from(&path).await.unwrap();
        let tiles_out = Arc::new(AsyncPmTilesReader::try_from_source(backend).await.unwrap());

        let header = tiles_out.get_header();
        assert_eq!(header.internal_compression, Compression::Gzip);

        let metadata_out = tiles_out.get_metadata().await.unwrap();
        assert_eq!(metadata_out, test_metadata);

        let entries = tiles_out
            .clone()
            .entries()
            .try_collect::<Vec<_>>()
            .await
            .unwrap();
        assert_eq!(entries.len(), 100);

        let tile_0 = tiles_out
            .get_tile_decompressed(TileId::new(0).unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(*tile_0, 0u64.to_le_bytes());
    }
}
