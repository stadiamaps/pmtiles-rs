use crate::Error;
use std::fmt::{Debug, Formatter};
use varint_rs::VarintReader;

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

impl TryFrom<&[u8]> for Directory {
    type Error = Error;

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
    use crate::{mmap::MmapBackend, AsyncBackend, AsyncPmTiles, Header};
    use std::path::Path;

    async fn create_backend() -> MmapBackend {
        MmapBackend::try_from(Path::new("fixtures/stamen_toner_z3.pmtiles"))
            .await
            .expect("Unable to open test file.")
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
}
