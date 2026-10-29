use std::fmt::{self, Debug, Formatter};
#[cfg(feature = "write")]
use std::io::{self, Write};

use bytes::Bytes;
use integer_encoding::VarInt as _;
#[cfg(feature = "write")]
use integer_encoding::VarIntWriter as _;

#[cfg(feature = "write")]
use crate::writer::WriteTo;
use crate::{MAX_TILE_ID, PmtError, PmtResult, TileId};

#[derive(Default, Clone)]
/// A directory of tile entries in a `PMTiles` file.
pub struct Directory {
    pub(crate) entries: Vec<DirEntry>,
}

impl Debug for Directory {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_fmt(format_args!("Directory [entries: {}]", self.entries.len()))
    }
}

impl Directory {
    #[cfg(feature = "write")]
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: Vec::with_capacity(capacity),
        }
    }

    #[cfg(feature = "write")]
    pub(crate) fn from_entries(entries: Vec<DirEntry>) -> Self {
        Self { entries }
    }

    #[cfg(feature = "write")]
    pub(crate) fn push(&mut self, entry: DirEntry) {
        self.entries.push(entry);
    }

    /// Find the directory entry for a given tile ID.
    #[must_use]
    pub fn find_tile_id(&self, tile_id: TileId) -> Option<&DirEntry> {
        match self
            .entries
            .binary_search_by(|e| e.tile_id.cmp(&tile_id.value()))
        {
            Ok(idx) => self.entries.get(idx),
            Err(next_id) => {
                // Adapted from JavaScript code at
                // https://github.com/protomaps/PMTiles/blob/9c7f298fb42290354b8ed0a9b2f50e5c0d270c40/js/index.ts#L210
                if next_id > 0 {
                    let previous_tile = self.entries.get(next_id - 1)?;
                    if previous_tile.is_leaf()
                        || (tile_id.value() - previous_tile.tile_id)
                            < u64::from(previous_tile.run_length)
                    {
                        return Some(previous_tile);
                    }
                }
                None
            }
        }
    }

    /// Get an estimated byte size of the directory object. Use this for cache eviction.
    #[must_use]
    pub fn get_approx_byte_size(&self) -> usize {
        self.entries.capacity() * size_of::<DirEntry>()
    }
}

/// Read a varint, rejecting truncated, overlong, or out-of-range values.
fn read_varint<T: TryFrom<u64>>(buf: &mut &[u8]) -> PmtResult<T> {
    let value = match buf.split_first() {
        // Most directory varints fit in a single byte
        Some((&byte, rest)) if byte < 0x80 => {
            *buf = rest;
            u64::from(byte)
        }
        _ => {
            let (value, len) = u64::decode_var(buf).ok_or(PmtError::InvalidEntry)?;
            *buf = buf.get(len..).ok_or(PmtError::InvalidEntry)?;
            value
        }
    };
    T::try_from(value).map_err(|_| PmtError::InvalidEntry)
}

impl TryFrom<Bytes> for Directory {
    type Error = PmtError;

    fn try_from(buffer: Bytes) -> Result<Self, Self::Error> {
        let mut buf: &[u8] = &buffer;
        let n_entries: usize = read_varint(&mut buf)?;
        // Each entry needs at least one byte for each of its four fields
        if n_entries > buf.len() / 4 {
            return Err(PmtError::InvalidEntry);
        }

        let mut entries = vec![DirEntry::default(); n_entries];

        // Read tile IDs
        let mut next_tile_id = 0_u64;
        for entry in &mut entries {
            next_tile_id = next_tile_id
                .checked_add(read_varint::<u64>(&mut buf)?)
                .ok_or(PmtError::InvalidEntry)?;
            entry.tile_id = next_tile_id;
        }

        // Read Run Lengths
        for entry in &mut entries {
            entry.run_length = read_varint(&mut buf)?;
            if !entry.is_leaf() {
                // Every tile id covered by the run must be a valid `TileId`
                let last_tile_id = entry
                    .tile_id
                    .checked_add(u64::from(entry.run_length - 1))
                    .ok_or(PmtError::InvalidEntry)?;
                if last_tile_id > MAX_TILE_ID {
                    return Err(PmtError::InvalidEntry);
                }
            }
        }

        // Read Lengths, which must not be 0
        for entry in &mut entries {
            entry.length = read_varint(&mut buf)?;
            if entry.length == 0 {
                return Err(PmtError::InvalidEntry);
            }
        }

        // Read Offsets
        let mut last_entry: Option<&DirEntry> = None;
        for entry in &mut entries {
            let offset: u64 = read_varint(&mut buf)?;
            entry.offset = if offset == 0 {
                let e = last_entry.ok_or(PmtError::InvalidEntry)?;
                e.offset
                    .checked_add(u64::from(e.length))
                    .ok_or(PmtError::InvalidEntry)?
            } else {
                offset - 1
            };
            last_entry = Some(entry);
        }

        Ok(Self { entries })
    }
}

#[cfg(feature = "write")]
impl WriteTo for Directory {
    fn write_to<W: Write>(&self, writer: &mut W) -> io::Result<()> {
        // Write number of entries
        writer.write_varint(self.entries.len())?;

        // Write tile IDs
        let mut last_tile_id = 0;
        for entry in &self.entries {
            writer.write_varint(entry.tile_id - last_tile_id)?;
            last_tile_id = entry.tile_id;
        }

        // Write Run Lengths
        for entry in &self.entries {
            writer.write_varint(entry.run_length)?;
        }

        // Write Lengths
        for entry in &self.entries {
            writer.write_varint(entry.length)?;
        }

        // Write Offsets
        let mut last_offset = 0;
        let mut last_length = 0;
        for (i, entry) in self.entries.iter().enumerate() {
            let offset_to_write = if i > 0 && entry.offset == last_offset + u64::from(last_length) {
                0
            } else {
                entry.offset + 1
            };
            writer.write_varint(offset_to_write)?;
            last_offset = entry.offset;
            last_length = entry.length;
        }

        Ok(())
    }
}

#[derive(Clone, Copy, Default, Debug, PartialEq)]
/// An entry in the `PMTiles` directory, representing a tile or a range of tiles.
pub struct DirEntry {
    pub(crate) tile_id: u64,
    pub(crate) offset: u64,
    pub(crate) length: u32,
    pub(crate) run_length: u32,
}

impl DirEntry {
    pub(crate) fn is_leaf(&self) -> bool {
        self.run_length == 0
    }

    #[cfg(feature = "iter-async")]
    #[must_use]
    /// Returns an iterator over the tile coordinates covered by this entry.
    pub fn iter_coords(&self) -> DirEntryCoordsIter<'_> {
        DirEntryCoordsIter {
            entry: self,
            current: 0,
        }
    }
}

#[cfg(feature = "iter-async")]
/// Iterator over tile coordinates in a directory entry.
pub struct DirEntryCoordsIter<'a> {
    entry: &'a DirEntry,
    current: u32,
}

#[cfg(feature = "iter-async")]
impl Iterator for DirEntryCoordsIter<'_> {
    type Item = TileId;

    #[expect(
        clippy::expect_used,
        reason = "parsing guarantees that the whole run fits within `MAX_TILE_ID`"
    )]
    fn next(&mut self) -> Option<Self::Item> {
        if self.current < self.entry.run_length {
            let current = u64::from(self.current);
            self.current += 1;
            // Parsing guarantees that the whole run fits within `MAX_TILE_ID`
            Some(TileId::new(self.entry.tile_id + current).expect("invalid entry data"))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::io::{BufReader, Read as _, Write as _};

    use bytes::{Bytes, BytesMut};
    use flate2::write::GzDecoder;
    use integer_encoding::VarIntWriter as _;
    use rstest::rstest;

    use crate::header::HEADER_SIZE;
    use crate::tests::RASTER_FILE;
    #[cfg(feature = "iter-async")]
    use crate::tile::test::coord;
    use crate::{Directory, Header, MAX_TILE_ID, PmtError};

    fn read_root_directory(file: &str) -> Directory {
        let test_file = File::open(file).unwrap();
        let mut reader = BufReader::new(test_file);

        let mut header_bytes = BytesMut::zeroed(HEADER_SIZE);
        reader.read_exact(header_bytes.as_mut()).unwrap();

        let header = Header::try_from_bytes(header_bytes.freeze()).unwrap();
        let mut directory_bytes = BytesMut::zeroed(usize::try_from(header.root_length).unwrap());
        reader.read_exact(directory_bytes.as_mut()).unwrap();

        let mut decompressed = BytesMut::zeroed(directory_bytes.len() * 2);
        {
            let mut gunzip = GzDecoder::new(decompressed.as_mut());
            gunzip.write_all(&directory_bytes).unwrap();
        }

        Directory::try_from(decompressed.freeze()).unwrap()
    }

    #[test]
    fn root_directory() {
        let directory = read_root_directory(RASTER_FILE);
        assert_eq!(directory.entries.len(), 84);
        // Note: this is not true for all tiles, just the first few...
        for nth in 0..10 {
            assert_eq!(directory.entries[nth].tile_id, nth as u64);
        }

        #[cfg(feature = "iter-async")]
        assert_eq!(
            directory.entries[57].iter_coords().collect::<Vec<_>>(),
            vec![coord(3, 4, 6).into()]
        );

        // ...it breaks the pattern on the 59th tile, because it has a run length of 2
        assert_eq!(directory.entries[58].tile_id, 58);
        assert_eq!(directory.entries[58].run_length, 2);
        assert_eq!(directory.entries[58].offset, 422_070);
        assert_eq!(directory.entries[58].length, 850);

        // that also means that it has two entries in xyz
        #[cfg(feature = "iter-async")]
        assert_eq!(
            directory.entries[58].iter_coords().collect::<Vec<_>>(),
            vec![coord(3, 4, 7).into(), coord(3, 5, 7).into()]
        );
    }

    #[rstest]
    #[case::tile_at_max(MAX_TILE_ID, 1, true)]
    #[case::run_ending_at_max(MAX_TILE_ID - 1, 2, true)]
    #[case::leaf_past_max(u64::MAX, 0, true)]
    #[case::tile_past_max(MAX_TILE_ID + 1, 1, false)]
    #[case::run_past_max(MAX_TILE_ID, 2, false)]
    #[case::run_overflows_u64(u64::MAX, 2, false)]
    fn entry_tile_range(#[case] tile_id: u64, #[case] run_length: u32, #[case] valid: bool) {
        // A directory with a single entry: count, tile id, run length, length, offset + 1
        let mut buf = vec![];
        buf.write_varint(1_usize).unwrap();
        buf.write_varint(tile_id).unwrap();
        buf.write_varint(run_length).unwrap();
        buf.write_varint(1_u32).unwrap();
        buf.write_varint(1_u64).unwrap();

        let result = Directory::try_from(Bytes::from(buf));
        if valid {
            let entries = result.unwrap().entries;
            assert_eq!(entries[0].tile_id, tile_id);
            assert_eq!(entries[0].run_length, run_length);
            #[cfg(feature = "iter-async")]
            assert_eq!(entries[0].iter_coords().count(), run_length as usize);
        } else {
            assert!(matches!(result, Err(PmtError::InvalidEntry)));
        }
    }

    #[rstest]
    #[case::too_many_entries(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x20])]
    #[case::varint_too_long(&[0xFF; 12])]
    #[case::varint_overflow(&[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x02])]
    #[case::length_over_u32(&[1, 0, 1, 0x80, 0x80, 0x80, 0x80, 0x20, 1])]
    #[case::zero_length(&[1, 0, 1, 0, 1])]
    #[case::tile_id_overflow(&[2, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x01, 1, 1, 1, 1, 1, 1, 1])]
    #[case::offset_overflow(&[2, 0, 1, 1, 1, 2, 1, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x01, 0])]
    fn malformed_directory(#[case] bytes: &[u8]) {
        let result = Directory::try_from(Bytes::copy_from_slice(bytes));
        assert!(matches!(result, Err(PmtError::InvalidEntry)), "{result:?}");
    }

    #[cfg(feature = "write")]
    mod write_tests {
        use super::*;
        use crate::{Compression, DirEntry, TileId};

        fn new_dir_entry(tile_id: u64, offset: u64, length: u32, run_length: u32) -> DirEntry {
            DirEntry {
                tile_id,
                offset,
                length,
                run_length,
            }
        }

        #[test]
        fn write_directory() {
            use crate::writer::WriteTo as _;

            let root_dir = read_root_directory(RASTER_FILE);
            let mut buf = vec![];
            root_dir.write_to(&mut buf).unwrap();
            let dir = Directory::try_from(Bytes::from(buf)).unwrap();
            assert!(root_dir.entries.iter().enumerate().all(
                |(idx, entry)| dir.entries[idx].tile_id == entry.tile_id
                    && dir.entries[idx].run_length == entry.run_length
                    && dir.entries[idx].offset == entry.offset
                    && dir.entries[idx].length == entry.length
            ));
        }

        fn roundtrip(entries: Vec<DirEntry>, compression: Compression) -> Vec<DirEntry> {
            use std::io::Read as _;

            use flate2::read::GzDecoder;

            use crate::writer::{Compressor, GzipCompressor, NoCompression, WriteTo as _};

            let dir = Directory::from_entries(entries);
            let mut buf = vec![];
            let compressor: &dyn Compressor = match compression {
                Compression::Gzip => &GzipCompressor::default(),
                Compression::None => &NoCompression,
                Compression::Unknown | Compression::Brotli | Compression::Zstd => {
                    panic!("This compression not handled by tests (yet)")
                }
            };
            dir.write_compressed_to(&mut buf, compressor).unwrap();

            let raw = match compression {
                Compression::Gzip => {
                    let mut decoded = vec![];
                    GzDecoder::new(buf.as_slice())
                        .read_to_end(&mut decoded)
                        .unwrap();
                    decoded
                }
                Compression::None => buf,
                Compression::Unknown | Compression::Brotli | Compression::Zstd => {
                    panic!("This compression not handled by tests (yet)")
                }
            };
            Directory::try_from(Bytes::from(raw)).unwrap().entries
        }

        // Ported from go-pmtiles/pmtiles/directory_test.go:TestDirectoryRoundtrip (line 10)
        // The first entry has length 1 instead of 0, which the spec does not allow
        #[test]
        fn directory_roundtrip_gzip() {
            let entries = vec![
                new_dir_entry(0, 0, 1, 0),
                new_dir_entry(1, 1, 1, 1),
                new_dir_entry(2, 2, 2, 2),
            ];
            let result = roundtrip(entries, Compression::Gzip);
            assert_eq!(result.len(), 3);
            assert_eq!(result[0], new_dir_entry(0, 0, 1, 0));
            assert_eq!(result[1], new_dir_entry(1, 1, 1, 1));
            assert_eq!(result[2], new_dir_entry(2, 2, 2, 2));
        }

        // Ported from go-pmtiles/pmtiles/directory_test.go:TestDirectoryRoundtripNoCompress (line 33)
        // The first entry has length 1 instead of 0, which the spec does not allow
        #[test]
        fn directory_roundtrip_no_compression() {
            let entries = vec![
                new_dir_entry(0, 0, 1, 0),
                new_dir_entry(1, 1, 1, 1),
                new_dir_entry(2, 2, 2, 2),
            ];
            let result = roundtrip(entries, Compression::None);
            assert_eq!(result.len(), 3);
            assert_eq!(result[0], new_dir_entry(0, 0, 1, 0));
            assert_eq!(result[1], new_dir_entry(1, 1, 1, 1));
            assert_eq!(result[2], new_dir_entry(2, 2, 2, 2));
        }

        // Ported from go-pmtiles/pmtiles/directory_test.go:TestFindTileMissing (line 157)
        #[test]
        fn find_tile_missing() {
            let dir = Directory::from_entries(vec![]);
            assert!(dir.find_tile_id(TileId::new(0).unwrap()).is_none());
        }

        // Ported from go-pmtiles/pmtiles/directory_test.go:TestFindTileFirstEntry (line 163)
        #[test]
        fn find_tile_first_entry() {
            let dir = Directory::from_entries(vec![new_dir_entry(100, 1, 1, 1)]);
            let entry = dir.find_tile_id(TileId::new(100).unwrap()).unwrap();
            assert_eq!(entry.offset, 1);
            assert_eq!(entry.length, 1);
            assert!(dir.find_tile_id(TileId::new(101).unwrap()).is_none());
        }

        // Ported from go-pmtiles/pmtiles/directory_test.go:TestFindTileMultipleEntries (line 173)
        #[test]
        fn find_tile_multiple_entries() {
            // Tile found via run_length on a single entry
            let dir = Directory::from_entries(vec![new_dir_entry(100, 1, 1, 2)]);
            let entry = dir.find_tile_id(TileId::new(101).unwrap()).unwrap();
            assert_eq!(entry.offset, 1);
            assert_eq!(entry.length, 1);

            // Tile found via run_length on a later entry
            let dir = Directory::from_entries(vec![
                new_dir_entry(100, 1, 1, 1),
                new_dir_entry(150, 2, 2, 2),
            ]);
            let entry = dir.find_tile_id(TileId::new(151).unwrap()).unwrap();
            assert_eq!(entry.offset, 2);
            assert_eq!(entry.length, 2);

            // Tile found via run_length on the first of three entries
            let dir = Directory::from_entries(vec![
                new_dir_entry(50, 1, 1, 2),
                new_dir_entry(100, 2, 2, 1),
                new_dir_entry(150, 3, 3, 1),
            ]);
            let entry = dir.find_tile_id(TileId::new(51).unwrap()).unwrap();
            assert_eq!(entry.offset, 1);
            assert_eq!(entry.length, 1);
        }

        // Ported from go-pmtiles/pmtiles/directory_test.go:TestFindTileLeafSearch (line 202)
        #[test]
        fn find_tile_leaf_search() {
            // run_length == 0 marks a leaf directory pointer; find_tile_id returns
            // the leaf entry for any tile_id past it so the caller can fetch that leaf.
            let dir = Directory::from_entries(vec![new_dir_entry(100, 1, 1, 0)]);
            let entry = dir.find_tile_id(TileId::new(150).unwrap()).unwrap();
            assert_eq!(entry.offset, 1);
            assert_eq!(entry.length, 1);
        }
    }
}
