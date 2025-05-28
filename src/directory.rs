use std::fmt::{Debug, Formatter};

use bytes::{Buf, Bytes};
use varint_rs::VarintReader as _;

use crate::error::PmtError;

#[derive(Clone)]
pub struct Directory {
    pub entries: Vec<DirEntry>,
}

impl Debug for Directory {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!("Directory [entries: {}]", self.entries.len()))
    }
}

impl Directory {
    /// Find the directory entry for a given tile ID.
    #[must_use]
    pub fn find_tile_id(&self, tile_id: u64) -> Option<&DirEntry> {
        match self.entries.binary_search_by(|e| e.tile_id.cmp(&tile_id)) {
            Ok(idx) => self.entries.get(idx),
            Err(next_id) => {
                // Adapted from javascript code at
                // https://github.com/protomaps/PMTiles/blob/9c7f298fb42290354b8ed0a9b2f50e5c0d270c40/js/index.ts#L210
                if next_id > 0 {
                    let previous_tile = self.entries.get(next_id - 1)?;
                    if previous_tile.is_leaf()
                        || tile_id - previous_tile.tile_id < u64::from(previous_tile.run_length)
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

    #[must_use]
    pub fn xyz(&self) -> Vec<(u8, u64, u64)> {
        // Create a vec of (z, x, y) tuples using run_length
        let mut xyz = Vec::with_capacity(self.run_length as usize);
        for i in 0..self.run_length {
            xyz.push(crate::tile::xyz(self.tile_id + u64::from(i)));
        }
        xyz
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufReader, Read, Write};
    use std::vec;

    use bytes::BytesMut;

    use super::Directory;
    use crate::header::HEADER_SIZE;
    use crate::tests::RASTER_FILE;
    use crate::Header;

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

        assert_eq!(directory.entries[57].xyz(), vec![(3, 4, 6)]);

        // ...it breaks pattern on the 59th tile, because it has a run length of 2
        assert_eq!(directory.entries[58].tile_id, 58);
        assert_eq!(directory.entries[58].run_length, 2);
        assert_eq!(directory.entries[58].offset, 422_070);
        assert_eq!(directory.entries[58].length, 850);
        // that also means that it has two entries in xyz
        assert_eq!(directory.entries[58].xyz(), vec![(3, 4, 7), (3, 5, 7)]);
    }
}
