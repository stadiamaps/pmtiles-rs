use std::fmt::{Debug, Formatter};

use bytes::{Buf, Bytes};
use varint_rs::VarintReader as _;
#[cfg(feature = "write")]
use varint_rs::VarintWriter as _;

use crate::{PmtError, TileId};

#[derive(Default, Clone)]
pub struct Directory {
    pub(crate) entries: Vec<DirEntry>,
}

impl Debug for Directory {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
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

impl TryFrom<Bytes> for Directory {
    type Error = PmtError;

    fn try_from(buffer: Bytes) -> Result<Self, Self::Error> {
        let mut buffer = buffer.reader();
        let n_entries = buffer.read_usize_varint()?;

        let mut entries = vec![DirEntry::default(); n_entries];

        // Read tile IDs
        let mut next_tile_id = 0;
        for entry in &mut entries {
            next_tile_id += buffer.read_u64_varint()?;
            entry.tile_id = next_tile_id;
        }

        // Read Run Lengths
        for entry in &mut entries {
            entry.run_length = buffer.read_u32_varint()?;
        }

        // Read Lengths
        for entry in &mut entries {
            entry.length = buffer.read_u32_varint()?;
        }

        // Read Offsets
        let mut last_entry: Option<&DirEntry> = None;
        for entry in &mut entries {
            let offset = buffer.read_u64_varint()?;
            entry.offset = if offset == 0 {
                let e = last_entry.ok_or(PmtError::InvalidEntry)?;
                e.offset + u64::from(e.length)
            } else {
                offset - 1
            };
            last_entry = Some(entry);
        }

        Ok(Directory { entries })
    }
}

#[cfg(feature = "write")]
impl crate::writer::WriteTo for Directory {
    fn write_to<W: std::io::Write>(&self, writer: &mut W) -> std::io::Result<()> {
        // Write number of entries
        writer.write_usize_varint(self.entries.len())?;

        // Write tile IDs
        let mut last_tile_id = 0;
        for entry in &self.entries {
            writer.write_u64_varint(entry.tile_id - last_tile_id)?;
            last_tile_id = entry.tile_id;
        }

        // Write Run Lengths
        for entry in &self.entries {
            writer.write_u32_varint(entry.run_length)?;
        }

        // Write Lengths
        for entry in &self.entries {
            writer.write_u32_varint(entry.length)?;
        }

        // Write Offsets
        let mut last_offset = 0;
        for entry in &self.entries {
            let offset_to_write = if entry.offset == last_offset + u64::from(entry.length) {
                0
            } else {
                entry.offset + 1
            };
            writer.write_u64_varint(offset_to_write)?;
            last_offset = entry.offset;
        }

        Ok(())
    }
}

#[derive(Clone, Default, Debug)]
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
    pub fn iter_coords(&self) -> DirEntryCoordsIter<'_> {
        DirEntryCoordsIter {
            entry: self,
            current: 0,
        }
    }
}

#[cfg(feature = "iter-async")]
pub struct DirEntryCoordsIter<'a> {
    entry: &'a DirEntry,
    current: u32,
}

#[cfg(feature = "iter-async")]
impl Iterator for DirEntryCoordsIter<'_> {
    type Item = TileId;

    fn next(&mut self) -> Option<Self::Item> {
        if self.current < self.entry.run_length {
            let current = u64::from(self.current);
            self.current += 1;
            Some(TileId::new(self.entry.tile_id + current).expect("invalid entry data"))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufReader, Read, Write};

    use bytes::BytesMut;

    use crate::header::HEADER_SIZE;
    use crate::tests::RASTER_FILE;
    #[cfg(feature = "iter-async")]
    use crate::tile::test::coord;
    use crate::{Directory, Header};

    fn read_root_directory(file: &str) -> Directory {
        let test_file = std::fs::File::open(file).unwrap();
        let mut reader = BufReader::new(test_file);

        let mut header_bytes = BytesMut::zeroed(HEADER_SIZE);
        reader.read_exact(header_bytes.as_mut()).unwrap();

        let header = Header::try_from_bytes(header_bytes.freeze()).unwrap();
        let mut directory_bytes = BytesMut::zeroed(usize::try_from(header.root_length).unwrap());
        reader.read_exact(directory_bytes.as_mut()).unwrap();

        let mut decompressed = BytesMut::zeroed(directory_bytes.len() * 2);
        {
            let mut gunzip = flate2::write::GzDecoder::new(decompressed.as_mut());
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

    #[test]
    #[cfg(feature = "write")]
    fn write_directory() {
        use crate::writer::WriteTo as _;

        let root_dir = read_root_directory(RASTER_FILE);
        let mut buf = vec![];
        root_dir.write_to(&mut buf).unwrap();
        let dir = Directory::try_from(bytes::Bytes::from(buf)).unwrap();
        assert!(
            root_dir
                .entries
                .iter()
                .enumerate()
                .all(|(idx, entry)| dir.entries[idx].tile_id == entry.tile_id
                    && dir.entries[idx].run_length == entry.run_length
                    && dir.entries[idx].offset == entry.offset
                    && dir.entries[idx].length == entry.length)
        );
    }
}
