//! Convert other tile sources into `PMTiles` archives.
//!
//! This module implements part of the crate roadmap: *conversion to `PMTiles`
//! from `MBTiles` and `x/y/z` tile directories*.
//!
//! Entry points:
//! - [`tile_dir_to_pmtiles`] — convert a directory of `z/x/y.<ext>` tiles.
//! - [`mbtiles_to_pmtiles`] — convert an `MBTiles` (`SQLite`) archive (requires the `mbtiles` feature).
//!
//! Both stream tiles into the archive in `PMTiles` (Hilbert) `TileId` order, so
//! the output is *clustered* and benefits from the writer's run-length and
//! content de-duplication. Only tile *keys* are held in memory during a
//! conversion; tile *data* is streamed one tile at a time.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs::File;
use std::path::{Path, PathBuf};

use crate::{
    Compression, MAX_ZOOM, PmTilesWriter, PmtError, PmtResult, TileCoord, TileId, TileType,
};

#[cfg(feature = "mbtiles")]
mod mbtiles;
#[cfg(feature = "mbtiles")]
pub use mbtiles::mbtiles_to_pmtiles;

/// Geographic bounds as `(min_lon, min_lat, max_lon, max_lat)` in degrees.
type LonLatBounds = (f64, f64, f64, f64);
/// A center point as `(lon, lat, zoom)`.
type CenterPoint = (f64, f64, u8);

/// The numbering scheme used for the `y` axis of a tile source.
///
/// `PMTiles` always uses [`Xyz`](TileScheme::Xyz) internally; this enum
/// describes how a *source's* `y` value should be interpreted so it can be
/// converted correctly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileScheme {
    /// `XYZ` ("Google"/"slippy") scheme, where `y` increases southward.
    /// This is what `PMTiles` uses, and what `gdal2tiles --xyz` emits.
    Xyz,
    /// `TMS` scheme, where `y` increases northward. This is what `MBTiles`
    /// stores, and what GDAL's default `gdal2tiles` mercator output uses.
    Tms,
}

/// Statistics describing a completed conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct ConvertStats {
    /// Number of tiles read from the source.
    pub tiles_read: u64,
    /// Number of (non-empty) tiles written to the archive.
    pub tiles_written: u64,
    /// Minimum zoom level present in the source, if any tiles were found.
    pub min_zoom: Option<u8>,
    /// Maximum zoom level present in the source, if any tiles were found.
    pub max_zoom: Option<u8>,
}

/// A single tile location, with its `PMTiles` id precomputed for sorting and a
/// source-specific token (`src`) used to fetch the tile bytes later.
struct TileKey<K> {
    id: u64,
    coord: TileCoord,
    src: K,
}

/// Accumulates the geographic extent of the converted tiles so that header
/// bounds, center and zoom range can be derived when the source does not
/// provide them explicitly.
#[derive(Default)]
struct Coverage {
    /// zoom level -> `[min_x, max_x, min_y, max_y]` in `XYZ` tile coordinates.
    zoom_extent: BTreeMap<u8, [u32; 4]>,
}

impl Coverage {
    fn observe(&mut self, z: u8, x: u32, y: u32) {
        let e = self.zoom_extent.entry(z).or_insert([x, x, y, y]);
        e[0] = e[0].min(x);
        e[1] = e[1].max(x);
        e[2] = e[2].min(y);
        e[3] = e[3].max(y);
    }

    fn min_zoom(&self) -> Option<u8> {
        self.zoom_extent.keys().next().copied()
    }

    fn max_zoom(&self) -> Option<u8> {
        self.zoom_extent.keys().next_back().copied()
    }

    /// Geographic bounds `(min_lon, min_lat, max_lon, max_lat)` derived from the
    /// tile coverage at the finest (maximum) zoom level present.
    fn bounds(&self) -> Option<LonLatBounds> {
        let (&z, e) = self.zoom_extent.iter().next_back()?;
        let west = tile_x_to_lon(e[0], z);
        let east = tile_x_to_lon(e[1] + 1, z);
        let north = tile_y_to_lat(e[2], z);
        let south = tile_y_to_lat(e[3] + 1, z);
        Some((west, south, east, north))
    }

    /// Bounds plus a derived center point `(lon, lat, zoom)`.
    fn bounds_and_center(&self) -> (Option<LonLatBounds>, Option<CenterPoint>) {
        match self.bounds() {
            Some(b @ (w, s, e, n)) => {
                let center = (
                    f64::midpoint(w, e),
                    f64::midpoint(s, n),
                    self.min_zoom().unwrap_or(0),
                );
                (Some(b), Some(center))
            }
            None => (None, None),
        }
    }
}

/// Convert a directory of `z/x/y.<ext>` tiles into a `PMTiles` archive.
///
/// `tile_type` declares the encoding of the tiles (e.g. [`TileType::Webp`]); the
/// bytes are stored verbatim — no re-encoding is performed. `scheme` selects how
/// the `y` filename is interpreted: use [`TileScheme::Tms`] for GDAL
/// `gdal2tiles` mercator output, or [`TileScheme::Xyz`] for "slippy"/Google
/// tiles.
///
/// Bounds, center and the zoom range are derived from the tiles present.
/// Non-numeric directory and file names are ignored, so sidecar files such as
/// `tilemapresource.xml` or `*.aux.xml` are skipped automatically.
///
/// # Errors
///
/// Returns an error if the directory cannot be read, a numerically-named tile
/// encodes an out-of-range coordinate, or writing the archive fails.
pub fn tile_dir_to_pmtiles(
    tile_dir: impl AsRef<Path>,
    dst: impl AsRef<Path>,
    tile_type: TileType,
    scheme: TileScheme,
) -> PmtResult<ConvertStats> {
    let tile_dir = tile_dir.as_ref();
    let mut keys: Vec<TileKey<PathBuf>> = Vec::new();
    let mut coverage = Coverage::default();

    for z_entry in std::fs::read_dir(tile_dir)? {
        let z_entry = z_entry?;
        if !z_entry.file_type()?.is_dir() {
            continue;
        }
        let Some(z) = parse_u32(&z_entry.file_name()).and_then(|v| u8::try_from(v).ok()) else {
            continue;
        };
        if z > MAX_ZOOM {
            continue;
        }

        for x_entry in std::fs::read_dir(z_entry.path())? {
            let x_entry = x_entry?;
            if !x_entry.file_type()?.is_dir() {
                continue;
            }
            let Some(x) = parse_u32(&x_entry.file_name()) else {
                continue;
            };

            for y_entry in std::fs::read_dir(x_entry.path())? {
                let y_entry = y_entry?;
                if !y_entry.file_type()?.is_file() {
                    continue;
                }
                let path = y_entry.path();
                let Some(y_raw) = path.file_stem().and_then(parse_u32) else {
                    continue;
                };
                let y = match scheme {
                    TileScheme::Xyz => y_raw,
                    TileScheme::Tms => flip_tms_xyz(z, y_raw)?,
                };
                let coord = TileCoord::new(z, x, y)?;
                coverage.observe(z, x, y);
                keys.push(TileKey {
                    id: u64::from(TileId::from(coord)),
                    coord,
                    src: path,
                });
            }
        }
    }

    // Raster tiles are stored as-is. Vector tiles are often gzip-compressed on
    // disk; sniff the first one so the header advertises the right encoding.
    let tile_compression = if matches!(tile_type, TileType::Mvt) {
        match keys.first() {
            Some(k) => gzip_compression(&std::fs::read(&k.src)?),
            None => Compression::None,
        }
    } else {
        Compression::None
    };

    let (bounds, center) = coverage.bounds_and_center();
    build_archive(
        dst.as_ref(),
        &ArchiveMeta {
            tile_type,
            tile_compression,
            metadata: "{}",
            bounds,
            center,
            min_zoom: coverage.min_zoom(),
            max_zoom: coverage.max_zoom(),
        },
        keys,
        |path| Ok(std::fs::read(path)?),
    )
}

/// Header-level information for the archive being written, derived from the
/// source before any tile data is streamed.
struct ArchiveMeta<'a> {
    tile_type: TileType,
    tile_compression: Compression,
    metadata: &'a str,
    bounds: Option<LonLatBounds>,
    center: Option<CenterPoint>,
    min_zoom: Option<u8>,
    max_zoom: Option<u8>,
}

/// Build a `PMTiles` archive from a set of tile keys and a `fetch` callback that
/// returns the bytes for each key. Keys are sorted into `TileId` order before
/// writing so the archive is clustered.
fn build_archive<K, F>(
    dst: &Path,
    meta: &ArchiveMeta<'_>,
    mut keys: Vec<TileKey<K>>,
    mut fetch: F,
) -> PmtResult<ConvertStats>
where
    F: FnMut(&K) -> PmtResult<Vec<u8>>,
{
    keys.sort_unstable_by_key(|k| k.id);

    let mut builder = PmTilesWriter::new(meta.tile_type)
        .tile_compression(meta.tile_compression)
        .metadata(meta.metadata);
    if let Some(z) = meta.min_zoom {
        builder = builder.min_zoom(z);
    }
    if let Some(z) = meta.max_zoom {
        builder = builder.max_zoom(z);
    }
    if let Some((min_lon, min_lat, max_lon, max_lat)) = meta.bounds {
        builder = builder.bounds(min_lon, min_lat, max_lon, max_lat);
    }
    if let Some((lon, lat, zoom)) = meta.center {
        builder = builder.center_zoom(zoom).center(lon, lat);
    }

    let mut writer = builder.create(File::create(dst)?)?;

    let tiles_read = keys.len() as u64;
    let mut tiles_written = 0u64;
    for key in &keys {
        let data = fetch(&key.src)?;
        if data.is_empty() {
            // The spec does not allow storing empty tiles; the writer ignores
            // them too, but skip explicitly so the count stays accurate.
            continue;
        }
        writer.add_raw_tile(key.coord, &data)?;
        tiles_written += 1;
    }
    writer.finalize()?;

    Ok(ConvertStats {
        tiles_read,
        tiles_written,
        min_zoom: meta.min_zoom,
        max_zoom: meta.max_zoom,
    })
}

/// Convert a `TMS` `y` value to the `XYZ` value `PMTiles` uses, validating the
/// coordinate against the zoom level.
fn flip_tms_xyz(z: u8, y: u32) -> PmtResult<u32> {
    let side = 1u32
        .checked_shl(u32::from(z))
        .ok_or(PmtError::InvalidCoordinate(z, 0, y))?;
    if y >= side {
        return Err(PmtError::InvalidCoordinate(z, 0, y));
    }
    Ok(side - 1 - y)
}

/// Returns [`Compression::Gzip`] if the sample begins with the gzip magic bytes,
/// otherwise [`Compression::None`].
fn gzip_compression(sample: &[u8]) -> Compression {
    if sample.starts_with(&[0x1f, 0x8b]) {
        Compression::Gzip
    } else {
        Compression::None
    }
}

fn parse_u32(name: &OsStr) -> Option<u32> {
    name.to_str()?.parse().ok()
}

/// Longitude (degrees) of the left edge of `XYZ` tile column `x` at zoom `z`.
fn tile_x_to_lon(x: u32, z: u8) -> f64 {
    let n = f64::from(1u32 << u32::from(z));
    f64::from(x) / n * 360.0 - 180.0
}

/// Latitude (degrees) of the top edge of `XYZ` tile row `y` at zoom `z`.
fn tile_y_to_lat(y: u32, z: u8) -> f64 {
    let n = f64::from(1u32 << u32::from(z));
    let lat_rad = (std::f64::consts::PI * (1.0 - 2.0 * f64::from(y) / n))
        .sinh()
        .atan();
    lat_rad.to_degrees()
}

#[cfg(test)]
#[cfg(feature = "mmap-async-tokio")]
mod tests {
    use std::path::Path;

    use tempfile::TempDir;

    use super::{TileScheme, tile_dir_to_pmtiles};
    use crate::{AsyncPmTilesReader, Compression, MmapBackend, TileCoord, TileType};

    fn write_tile(root: &Path, z: u8, x: u32, y: u32, data: &[u8]) {
        let dir = root.join(z.to_string()).join(x.to_string());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{y}.png")), data).unwrap();
    }

    #[tokio::test]
    async fn tms_dir_roundtrip_flips_y() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("tiles");
        // z=1 TMS rows: row 0 is the southern row, row 1 the northern row.
        write_tile(&dir, 1, 0, 0, b"sw");
        write_tile(&dir, 1, 0, 1, b"nw");
        write_tile(&dir, 0, 0, 0, b"root");
        // A sidecar file that must be ignored.
        std::fs::write(dir.join("1").join("0").join("tilemapresource.xml"), b"x").unwrap();

        let out = tmp.path().join("out.pmtiles");
        let stats = tile_dir_to_pmtiles(&dir, &out, TileType::Png, TileScheme::Tms).unwrap();
        assert_eq!(stats.tiles_read, 3);
        assert_eq!(stats.tiles_written, 3);
        assert_eq!(stats.min_zoom, Some(0));
        assert_eq!(stats.max_zoom, Some(1));

        let backend = MmapBackend::try_from(out.to_str().unwrap()).await.unwrap();
        let reader = AsyncPmTilesReader::try_from_source(backend).await.unwrap();
        assert_eq!(reader.get_header().tile_type, TileType::Png);
        assert_eq!(reader.get_header().tile_compression, Compression::None);

        // TMS (1,0,0) (south) maps to XYZ (1,0,1).
        let t = reader
            .get_tile(TileCoord::new(1, 0, 1).unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&*t, b"sw");
        // TMS (1,0,1) (north) maps to XYZ (1,0,0).
        let t = reader
            .get_tile(TileCoord::new(1, 0, 0).unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&*t, b"nw");
    }

    #[tokio::test]
    async fn xyz_dir_no_flip() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("tiles");
        write_tile(&dir, 1, 0, 0, b"a");

        let out = tmp.path().join("out.pmtiles");
        tile_dir_to_pmtiles(&dir, &out, TileType::Png, TileScheme::Xyz).unwrap();

        let backend = MmapBackend::try_from(out.to_str().unwrap()).await.unwrap();
        let reader = AsyncPmTilesReader::try_from_source(backend).await.unwrap();
        let t = reader
            .get_tile(TileCoord::new(1, 0, 0).unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&*t, b"a");
    }

    #[test]
    fn out_of_range_coordinate_errors() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("tiles");
        // x=9 is invalid at z=1 (only 0..=1 allowed).
        write_tile(&dir, 1, 9, 0, b"bad");
        let out = tmp.path().join("out.pmtiles");
        let err = tile_dir_to_pmtiles(&dir, &out, TileType::Png, TileScheme::Xyz);
        assert!(matches!(
            err,
            Err(crate::PmtError::InvalidCoordinate(1, 9, 0))
        ));
    }

    #[test]
    fn flip_tms_xyz_validates() {
        use super::flip_tms_xyz;
        assert_eq!(flip_tms_xyz(0, 0).unwrap(), 0);
        assert_eq!(flip_tms_xyz(1, 0).unwrap(), 1);
        assert_eq!(flip_tms_xyz(1, 1).unwrap(), 0);
        assert!(flip_tms_xyz(1, 2).is_err()); // y out of range for z=1
    }
}
