//! Extract subsets of tiles from `PMTiles` archives.
//!
//! Extracts tiles within a bounding box, optimizing network requests through range merging.
//! Source archive must be **clustered** (tiles stored in Hilbert curve order).
//!
//! # Examples
//!
//! ## Extracting to a file
//!
//! ```no_run
//! use pmtiles::extract::{BoundingBox, Extractor};
//! use pmtiles::{AsyncPmTilesReader, MmapBackend};
//! use std::fs::File;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! // Open source archive
//! let backend = MmapBackend::try_from("source.pmtiles").await?;
//! let mut reader = AsyncPmTilesReader::try_from_source(backend).await?;
//!
//! // Create extractor
//! let extractor = Extractor::new(&mut reader);
//!
//! // Define bounding box (North, East, South, West)
//! let bbox = BoundingBox::from_nesw(37.8, -122.4, 37.7, -122.5);
//!
//! // Extract to file
//! let mut output = File::create("extracted.pmtiles")?;
//! let stats = extractor.extract_bbox_to_writer(bbox, &mut output).await?;
//!
//! println!("Extracted {} tiles", stats.addressed_tiles());
//! println!("Transferred {} bytes", stats.total_tile_transfer_bytes());
//! # Ok(())
//! # }
//! ```
//!
//! ## Checking extraction size before downloading
//!
//! ```no_run
//! use pmtiles::extract::{BoundingBox, Extractor};
//! use pmtiles::{AsyncPmTilesReader, MmapBackend};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! // Open source archive
//! let backend = MmapBackend::try_from("source.pmtiles").await?;
//! let mut reader = AsyncPmTilesReader::try_from_source(backend).await?;
//!
//! // Create extractor
//! let extractor = Extractor::new(&mut reader);
//!
//! let bbox = BoundingBox::from_nesw(37.8, -122.4, 37.7, -122.5);
//!
//! // Get size estimate without fetching tiles
//! let plan = extractor
//!     .prepare(bbox)
//!     .await?;
//!
//! println!("Would extract {} tiles using {} tile requests", plan.addressed_tiles(), plan.overfetch_ranges().len());
//! println!("Would transfer {} bytes", plan.total_tile_transfer_bytes());
//!
//! // Only proceed if reasonable size
//! if plan.total_tile_transfer_bytes() < 100_000_000 {
//!     // Proceed with actual extraction...
//! }
//! # Ok(())
//! # }
//! ```

mod bbox;
mod ranges;

mod extractor;
#[cfg(test)]
mod tests;

use std::collections::HashMap;

pub use bbox::BoundingBox;
pub use extractor::{ExtractProgressCallback, Extractor};
pub use ranges::{CopyDiscard, OverfetchRange, SrcDstRange, merge_ranges};
use roaring::RoaringTreemap;

use crate::{DirEntry, TileCoord, TileId};

/// Extraction plan from analyzing bbox and directories.
///
/// This contains all the information needed to compute extraction size
/// and perform the actual extraction.
#[derive(Debug, Clone)]
pub struct ExtractionPlan {
    pub(crate) stats: ExtractStats,
    pub(crate) reencoded_entries: Vec<DirEntry>,
    pub(crate) overfetch_ranges: Vec<OverfetchRange>,
}

/// Statistics about an extraction operation.
#[derive(Debug, Clone)]
pub struct ExtractStats {
    pub(crate) min_zoom: u8,
    pub(crate) max_zoom: u8,
    pub(crate) bbox: BoundingBox,
    pub(crate) total_tile_transfer_bytes: u64,
    pub(crate) tile_data_length: u64,
    pub(crate) addressed_tiles: u64,
    pub(crate) tile_contents: u64,
    pub(crate) num_leaf_entries: usize,
    pub(crate) num_tile_reqs: usize,
}

impl ExtractStats {
    /// Total bytes transferred (includes overfetch)
    #[must_use]
    pub fn total_tile_transfer_bytes(&self) -> u64 {
        self.total_tile_transfer_bytes
    }

    /// Actual tile data bytes used
    #[must_use]
    pub fn tile_data_length(&self) -> u64 {
        self.tile_data_length
    }

    /// Number of unique tile contents
    #[must_use]
    pub fn tile_contents(&self) -> u64 {
        self.tile_contents
    }

    /// Number of addressed tiles (reencoded entries)
    #[must_use]
    pub fn addressed_tiles(&self) -> u64 {
        self.addressed_tiles
    }

    /// Number of overfetch ranges used for fetching
    #[must_use]
    pub fn num_tile_reqs(&self) -> usize {
        self.num_tile_reqs
    }
}

impl ExtractionPlan {
    /// Re-encoded tile entries with new contiguous offsets
    #[must_use]
    pub fn reencoded_entries(&self) -> &[DirEntry] {
        &self.reencoded_entries
    }

    /// Merged byte ranges for optimized fetching
    #[must_use]
    pub fn overfetch_ranges(&self) -> &[OverfetchRange] {
        &self.overfetch_ranges
    }

    /// Minimum zoom level for extraction
    #[must_use]
    pub fn min_zoom(&self) -> u8 {
        self.stats.min_zoom
    }

    /// Maximum zoom level for extraction
    #[must_use]
    pub fn max_zoom(&self) -> u8 {
        self.stats.max_zoom
    }

    /// The bounding box that was extracted
    #[must_use]
    pub fn bbox(&self) -> BoundingBox {
        self.stats.bbox
    }

    /// Total bytes to transfer (including overfetch)
    #[must_use]
    pub fn total_tile_transfer_bytes(&self) -> u64 {
        self.stats.total_tile_transfer_bytes
    }

    /// Actual tile data bytes (excluding overfetch)
    #[must_use]
    pub fn tile_data_length(&self) -> u64 {
        self.stats.tile_data_length
    }

    /// Number of addressed tiles (sum of run lengths)
    #[must_use]
    pub fn addressed_tiles(&self) -> u64 {
        self.stats.addressed_tiles
    }

    /// Number of unique tile contents
    #[must_use]
    pub fn tile_contents(&self) -> u64 {
        self.stats.tile_contents
    }

    /// Number of leaf directory entries read
    #[must_use]
    pub fn num_leaf_entries(&self) -> usize {
        self.stats.num_leaf_entries
    }
}

/// Filters directory entries to those intersecting the bitmap.
///
/// Returns `(tile_entries, leaf_entries)`.
///
/// # Panics
///
/// Panics if `max_zoom + 1` is not a valid tile coordinate.
#[must_use]
pub fn relevant_entries(
    bitmap: &RoaringTreemap,
    max_zoom: u8,
    dir: &[DirEntry],
) -> (Vec<DirEntry>, Vec<DirEntry>) {
    // Calculate last tile ID for bounding leaf ranges
    let last_tile = if max_zoom < 31 {
        TileId::from(TileCoord::new(max_zoom + 1, 0, 0).expect("valid coord")).value()
    } else {
        // At max zoom, use max tile ID
        crate::tile::MAX_TILE_ID
    };

    let mut leaves = Vec::new();
    let mut tiles = Vec::new();

    for (idx, entry) in dir.iter().enumerate() {
        log::debug!("enumerating {idx}");
        if entry.run_length == 0 {
            let mut tmp = RoaringTreemap::new();

            if let Some(next) = dir.get(idx + 1) {
                tmp.insert_range(entry.tile_id..next.tile_id);
            } else {
                // Last entry - bounded by last_tile
                tmp.insert_range(entry.tile_id..last_tile);
            }

            if bitmap.is_disjoint(&tmp) {
                // No intersection
                continue;
            }
            leaves.push(entry.clone());
        } else if entry.run_length == 1 {
            if bitmap.contains(entry.tile_id) {
                tiles.push(entry.clone());
            }
        } else {
            // Run length > 1
            let mut current_id = entry.tile_id;
            let mut current_run_length = 0_u32;

            for tile_id in entry.tile_id..(entry.tile_id + u64::from(entry.run_length)) {
                if bitmap.contains(tile_id) {
                    if current_run_length == 0 {
                        current_run_length = 1;
                        current_id = tile_id;
                    } else {
                        current_run_length += 1;
                    }
                } else {
                    if current_run_length > 0 {
                        // End of a run
                        tiles.push(DirEntry {
                            tile_id: current_id,
                            offset: entry.offset,
                            length: entry.length,
                            run_length: current_run_length,
                        });
                    }
                    current_run_length = 0;
                }
            }

            if current_run_length > 0 {
                tiles.push(DirEntry {
                    tile_id: current_id,
                    offset: entry.offset,
                    length: entry.length,
                    run_length: current_run_length,
                });
            }
        }
    }

    (tiles, leaves)
}

/// Re-encodes entries with contiguous offsets, deduplicating tiles.
///
/// Returns (`entries`, `ranges`, `tile_data_length`, `addressed_tiles`, `tile_contents`).
///
/// Based on <https://github.com/protomaps/go-pmtiles/blob/f1c24e64f3085877d57c8e0f07233e0a3ef25a99/pmtiles/extract.go#L93>
#[must_use]
pub fn reencode_entries(dir: Vec<DirEntry>) -> (Vec<DirEntry>, Vec<SrcDstRange>, u64, u64, u64) {
    let mut reencoded = Vec::with_capacity(dir.len());
    let mut seen_offsets: HashMap<u64, u64> = HashMap::new();
    let mut ranges: Vec<SrcDstRange> = Vec::new();
    let mut addressed_tiles = 0_u64;
    let mut dst_offset = 0_u64;

    for entry in dir {
        addressed_tiles += u64::from(entry.run_length);

        if let Some(&existing_dst_offset) = seen_offsets.get(&entry.offset) {
            // We've seen this source offset before - reuse the destination offset (deduplication)
            reencoded.push(DirEntry {
                offset: existing_dst_offset,
                ..entry
            });
        } else {
            // New source offset - need to copy this data

            // Check if we can merge with the previous range
            if let Some(last) = ranges.last_mut()
                && last.src_end() == entry.offset
            {
                last.length += u64::from(entry.length);
            } else {
                ranges.push(SrcDstRange {
                    src_offset: entry.offset,
                    dst_offset,
                    length: u64::from(entry.length),
                });
            }

            reencoded.push(DirEntry {
                offset: dst_offset,
                ..entry
            });

            seen_offsets.insert(entry.offset, dst_offset);
            dst_offset += u64::from(entry.length);
        }
    }

    let tile_data_length = dst_offset;
    let tile_contents = seen_offsets.len() as u64;

    (
        reencoded,
        ranges,
        tile_data_length,
        addressed_tiles,
        tile_contents,
    )
}
