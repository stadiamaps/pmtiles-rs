use bytes::{Buf, Bytes};
use std::fmt::{Debug, Formatter};

use varint_rs::VarintReader;

use crate::error::Error;

pub(crate) struct Directory {
    entries: Vec<Entry>,
}

impl Debug for Directory {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!("Directory [entries: {}]", self.entries.len()))
    }
}

impl Directory {
    pub fn find_tile_id(&self, tile_id: u64) -> Option<&Entry> {
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

impl TryFrom<Bytes> for Directory {
    type Error = Error;

    fn try_from(buffer: Bytes) -> Result<Self, Error> {
        let mut buffer = buffer.reader();
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
}

#[derive(Clone, Default, Debug)]
pub(crate) struct Entry {
    pub(crate) tile_id: u64,
    pub(crate) offset: u64,
    pub(crate) length: u32,
    pub(crate) run_length: u32,
}

#[cfg(test)]
mod tests {
    use bytes::BytesMut;
    use std::io::Write;
    use std::{
        io::{BufReader, Read},
        path::Path,
    };

    use crate::header::HEADER_SIZE;
    use crate::Header;

    use super::Directory;

    #[test]
    fn read_root_directory() {
        let test_file = std::fs::File::open(Path::new(
            "fixtures/stamen_toner(raster)CC-BY+ODbL_z3.pmtiles",
        ))
        .expect("Unable to open fixture.");
        let mut reader = BufReader::new(test_file);

        let mut header_bytes = BytesMut::zeroed(HEADER_SIZE);
        reader
            .read_exact(header_bytes.as_mut())
            .expect("Unable to read header");

        let header =
            Header::try_from_bytes(header_bytes.freeze()).expect("Unable to parse header.");
        let mut directory_bytes = BytesMut::zeroed(header.root_length as usize);
        reader
            .read_exact(directory_bytes.as_mut())
            .expect("Unable to read root directory bytes.");

        let mut decompressed = BytesMut::zeroed(directory_bytes.len() * 2);
        {
            let mut gunzip = flate2::write::GzDecoder::new(decompressed.as_mut());
            gunzip
                .write_all(&directory_bytes)
                .expect("Unable to decompress");
        }

        let directory =
            Directory::try_from(decompressed.freeze()).expect("Unable to read directory");

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
}
