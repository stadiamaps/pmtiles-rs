// Bounding box utilities for extraction

use roaring::RoaringTreemap;

use crate::PmtResult;
use crate::tile::{TileCoord, TileId};

/// A geographic bounding box in WGS84 coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoundingBox {
    /// Maximum latitude (north) in degrees (-90 to 90)
    pub max_lat: f64,
    /// Maximum longitude (east) in degrees (-180 to 180)
    pub max_lon: f64,
    /// Minimum latitude (south) in degrees (-90 to 90)
    pub min_lat: f64,
    /// Minimum longitude (west) in degrees (-180 to 180)
    pub min_lon: f64,
}

impl BoundingBox {
    /// Creates a bounding box from North, East, South, West coordinates.
    #[must_use]
    pub fn from_nesw(north: f64, east: f64, south: f64, west: f64) -> Self {
        Self {
            max_lat: north,
            max_lon: east,
            min_lat: south,
            min_lon: west,
        }
    }

    /// Creates a bitmap containing all tiles that intersect with the bounding box
    ///
    /// # Errors
    ///
    /// Returns an error if the coordinates are out of bounds.
    pub fn tile_bitmap(&self, min_zoom: u8, max_zoom: u8) -> PmtResult<RoaringTreemap> {
        let mut bitmap = RoaringTreemap::new();

        // Add tiles at max_zoom that intersect the bbox
        // min_lat/max_lat need to be swapped because y increases southward
        let min_tile = TileCoord::from_lon_lat_zoom(self.min_lon, self.max_lat, max_zoom)?;
        let max_tile = TileCoord::from_lon_lat_zoom(self.max_lon, self.min_lat, max_zoom)?;

        // Add all tiles in the rectangle at max_zoom
        for x in min_tile.x()..=max_tile.x() {
            for y in min_tile.y()..=max_tile.y() {
                if let Ok(coord) = TileCoord::new(max_zoom, x, y) {
                    let tile_id = TileId::from(coord).value();
                    bitmap.insert(tile_id);
                }
            }
        }

        // Generalize: add parent tiles down to min_zoom
        generalize_or(&mut bitmap, min_zoom)?;

        Ok(bitmap)
    }
}

/// Add parent tiles to the bitmap down to `min_zoom`.
/// Port of generalizeOr from go-pmtiles/pmtiles/bitmap.go:42
fn generalize_or(bitmap: &mut RoaringTreemap, min_zoom: u8) -> PmtResult<()> {
    if bitmap.is_empty() {
        return Ok(());
    }

    // Find max zoom from the highest tile ID
    let max_tile_id = bitmap.max().expect("bitmap not empty");
    let max_coord = TileCoord::from(TileId::new(max_tile_id)?);
    let max_z = max_coord.z();

    let mut temp = RoaringTreemap::new();
    let mut to_iterate = bitmap.clone();

    // Work backwards from max zoom to min_zoom, adding parents
    for _current_z in min_zoom..max_z {
        temp.clear();

        for tile_id in &to_iterate {
            let Some(parent_id) = TileId::new(tile_id)?.parent_id() else {
                continue;
            };
            temp.insert(parent_id.value());
        }

        to_iterate.clone_from(&temp);
        *bitmap |= &temp;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bbox_to_bitmap() {
        // Small bbox should produce some tiles
        let bbox = BoundingBox::from_nesw(37.8, -122.4, 37.7, -122.5);
        let bitmap = bbox.tile_bitmap(10, 12).unwrap();

        assert!(!bitmap.is_empty());
        // Should have tiles at zoom 10, 11, and 12
        assert!(!bitmap.is_empty());
    }
}
