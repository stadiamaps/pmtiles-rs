//! Fuzz harnesses for the `pmtiles` reader.
//!
//! Each harness must return for any input. Errors are expected, but a panic, an abort
//! (e.g. a failed allocation), a hang or running out of memory is a bug.

use std::sync::Arc;

use bytes::Bytes;
use futures_executor::block_on;
use futures_util::TryStreamExt as _;
use pmtiles::{AsyncBackend, AsyncPmTilesReader, BackendResponse, Directory, PmtResult, TileId};

/// The most tiles read from one archive, so that each input stays fast.
const MAX_TILES: usize = 100;

/// Serves an archive from memory. A read past the end returns fewer bytes, or none.
struct MemoryBackend(Bytes);

impl AsyncBackend for MemoryBackend {
    fn read(
        &self,
        offset: u64,
        length: usize,
    ) -> impl Future<Output = PmtResult<BackendResponse>> + Send {
        let len = self.0.len();
        let start = usize::try_from(offset).map_or(len, |offset| offset.min(len));
        let end = start.saturating_add(length).min(len);
        std::future::ready(Ok(BackendResponse::new(self.0.slice(start..end))))
    }
}

/// Opens `data` as an archive, then reads its metadata, `TileJSON`, entries and tiles.
pub fn read_archive(data: &[u8]) {
    block_on(async {
        let backend = MemoryBackend(Bytes::copy_from_slice(data));
        let Ok(reader) = AsyncPmTilesReader::try_from_source(backend).await else {
            return;
        };
        let _ = reader.get_metadata().await;
        let _ = reader.parse_tilejson(Vec::new()).await;
        if let Ok(tile_id) = TileId::new(0) {
            let _ = reader.get_tile_decompressed(tile_id).await;
        }

        let reader = Arc::new(reader);
        let mut entries = reader.clone().entries();
        for _ in 0..MAX_TILES {
            let Ok(Some(entry)) = entries.try_next().await else {
                break;
            };
            if let Some(coord) = entry.iter_coords().next() {
                let _ = reader.get_tile_decompressed(coord).await;
            }
        }
    });
}

/// Parses `data` as an uncompressed directory and looks up a tile in it.
pub fn parse_directory(data: &[u8]) {
    if let (Ok(directory), Ok(tile_id)) = (
        Directory::try_from(Bytes::copy_from_slice(data)),
        TileId::new(0),
    ) {
        let _ = directory.find_tile_id(tile_id);
    }
}
