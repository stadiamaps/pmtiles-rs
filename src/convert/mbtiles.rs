//! `MBTiles` (`SQLite`) → `PMTiles` conversion.

use std::collections::HashMap;
use std::path::Path;

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde_json::{Map, Value};

use super::{ArchiveMeta, Coverage, build_archive, flip_tms_xyz, gzip_compression};
use crate::{Compression, ConvertStats, PmtError, PmtResult, TileCoord, TileId, TileType};

/// Keys that map onto `PMTiles` header fields rather than the JSON metadata blob.
const STRUCTURAL_METADATA_KEYS: [&str; 6] =
    ["bounds", "center", "minzoom", "maxzoom", "format", "json"];

/// Convert an `MBTiles` (`SQLite`) archive into a `PMTiles` archive.
///
/// `MBTiles` stores tile rows in the `TMS` scheme; they are flipped to the `XYZ`
/// scheme `PMTiles` uses. The tile type and compression are taken from the
/// `metadata` table's `format` field when present, and otherwise sniffed from
/// the tile bytes. Tile data is stored verbatim — gzip-compressed vector tiles
/// stay compressed and the header advertises [`Compression::Gzip`] accordingly.
///
/// Bounds, center and the zoom range are taken from the `metadata` table when
/// available, falling back to values derived from the tiles themselves.
///
/// # Errors
///
/// Returns an error if the database cannot be opened or queried, a tile encodes
/// an invalid coordinate, the metadata cannot be assembled into valid JSON, or
/// writing the archive fails.
pub fn mbtiles_to_pmtiles(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> PmtResult<ConvertStats> {
    let conn = Connection::open_with_flags(
        src.as_ref(),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )?;

    let meta = read_metadata(&conn)?;
    let sample: Option<Vec<u8>> = conn
        .query_row("SELECT tile_data FROM tiles LIMIT 1", [], |r| r.get(0))
        .optional()?;

    let tile_type = meta
        .get("format")
        .and_then(|f| tile_type_from_format(&f.to_ascii_lowercase()))
        .or_else(|| sample.as_deref().map(sniff_tile_type))
        .unwrap_or(TileType::Unknown);

    let tile_compression = match tile_type {
        TileType::Mvt => sample
            .as_deref()
            .map_or(Compression::None, gzip_compression),
        _ => Compression::None,
    };

    // Pass 1: collect tile keys (no blobs) and accumulate coverage.
    let mut keys = Vec::new();
    let mut coverage = Coverage::default();
    {
        let mut stmt = conn.prepare("SELECT zoom_level, tile_column, tile_row FROM tiles")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let z: i64 = row.get(0)?;
            let col: i64 = row.get(1)?;
            let tms: i64 = row.get(2)?;

            let z =
                u8::try_from(z).map_err(|_| PmtError::Conversion(format!("invalid zoom {z}")))?;
            let x = u32::try_from(col)
                .map_err(|_| PmtError::Conversion(format!("invalid tile_column {col}")))?;
            let tms = u32::try_from(tms)
                .map_err(|_| PmtError::Conversion(format!("invalid tile_row {tms}")))?;

            let y = flip_tms_xyz(z, tms)?;
            let coord = TileCoord::new(z, x, y)?;
            coverage.observe(z, x, y);
            keys.push(super::TileKey {
                id: u64::from(TileId::from(coord)),
                coord,
                src: (z, x, tms),
            });
        }
    }

    let metadata = build_metadata_json(&meta)?;
    let min_zoom = meta
        .get("minzoom")
        .and_then(|s| s.trim().parse::<u8>().ok())
        .or_else(|| coverage.min_zoom());
    let max_zoom = meta
        .get("maxzoom")
        .and_then(|s| s.trim().parse::<u8>().ok())
        .or_else(|| coverage.max_zoom());
    let bounds = meta
        .get("bounds")
        .and_then(|s| parse_bounds(s))
        .or_else(|| coverage.bounds());
    // Prefer an explicit center; otherwise derive one from whichever bounds we
    // resolved, so the two stay consistent.
    let center = meta
        .get("center")
        .and_then(|s| parse_center(s))
        .or_else(|| {
            bounds.map(|(w, s, e, n)| {
                (
                    f64::midpoint(w, e),
                    f64::midpoint(s, n),
                    min_zoom.unwrap_or(0),
                )
            })
        });

    // Pass 2: stream tile blobs in TileId order via an indexed point lookup.
    let mut fetch_stmt = conn.prepare(
        "SELECT tile_data FROM tiles WHERE zoom_level=?1 AND tile_column=?2 AND tile_row=?3",
    )?;
    let fetch = |src: &(u8, u32, u32)| -> PmtResult<Vec<u8>> {
        let (z, x, tms) = *src;
        let data: Option<Vec<u8>> = fetch_stmt
            .query_row(params![i64::from(z), i64::from(x), i64::from(tms)], |row| {
                row.get(0)
            })
            .optional()?;
        Ok(data.unwrap_or_default())
    };

    build_archive(
        dst.as_ref(),
        &ArchiveMeta {
            tile_type,
            tile_compression,
            metadata: &metadata,
            bounds,
            center,
            min_zoom,
            max_zoom,
        },
        keys,
        fetch,
    )
}

fn read_metadata(conn: &Connection) -> PmtResult<HashMap<String, String>> {
    let mut map = HashMap::new();
    // The metadata table is optional; treat a missing table as empty metadata.
    let Ok(mut stmt) = conn.prepare("SELECT name, value FROM metadata") else {
        return Ok(map);
    };
    let rows = stmt.query_map([], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
    })?;
    for row in rows {
        let (name, value) = row?;
        if let Some(value) = value {
            map.insert(name, value);
        }
    }
    Ok(map)
}

/// Build the `PMTiles` JSON metadata object from the `MBTiles` `metadata` table.
///
/// The contents of the `json` field (if it is a JSON object, e.g. carrying
/// `vector_layers`) are merged in, and all non-structural string entries are
/// preserved. Keys that map onto header fields are dropped from the JSON blob.
fn build_metadata_json(meta: &HashMap<String, String>) -> PmtResult<String> {
    let mut obj = Map::new();

    if let Some(raw) = meta.get("json")
        && let Ok(Value::Object(inner)) = serde_json::from_str::<Value>(raw)
    {
        for (k, v) in inner {
            obj.insert(k, v);
        }
    }

    for (k, v) in meta {
        if STRUCTURAL_METADATA_KEYS.contains(&k.as_str()) {
            continue;
        }
        obj.entry(k.clone())
            .or_insert_with(|| Value::String(v.clone()));
    }

    serde_json::to_string(&Value::Object(obj))
        .map_err(|e| PmtError::Conversion(format!("building metadata JSON failed: {e}")))
}

fn tile_type_from_format(format: &str) -> Option<TileType> {
    Some(match format {
        "pbf" | "mvt" => TileType::Mvt,
        "png" => TileType::Png,
        "jpg" | "jpeg" => TileType::Jpeg,
        "webp" => TileType::Webp,
        "avif" => TileType::Avif,
        _ => return None,
    })
}

/// Best-effort detection of the tile type from the leading bytes of a tile.
fn sniff_tile_type(sample: &[u8]) -> TileType {
    if sample.starts_with(&[0x89, b'P', b'N', b'G']) {
        TileType::Png
    } else if sample.starts_with(&[0xff, 0xd8]) {
        TileType::Jpeg
    } else if sample.len() >= 12 && &sample[0..4] == b"RIFF" && &sample[8..12] == b"WEBP" {
        TileType::Webp
    } else if sample.starts_with(&[0x1f, 0x8b]) {
        // gzip — almost certainly a compressed vector tile.
        TileType::Mvt
    } else {
        TileType::Unknown
    }
}

fn parse_bounds(s: &str) -> Option<(f64, f64, f64, f64)> {
    let parts: Vec<f64> = s.split(',').filter_map(|t| t.trim().parse().ok()).collect();
    match parts.as_slice() {
        [w, s, e, n] => Some((*w, *s, *e, *n)),
        _ => None,
    }
}

fn parse_center(s: &str) -> Option<(f64, f64, u8)> {
    let mut parts = s.split(',');
    let lon = parts.next()?.trim().parse().ok()?;
    let lat = parts.next()?.trim().parse().ok()?;
    let zoom = parts
        .next()
        .and_then(|p| p.trim().parse::<u8>().ok())
        .unwrap_or(0);
    Some((lon, lat, zoom))
}

#[cfg(test)]
#[cfg(feature = "mmap-async-tokio")]
mod tests {
    use rusqlite::{Connection, params};
    use tempfile::TempDir;

    use super::mbtiles_to_pmtiles;
    use crate::{AsyncPmTilesReader, Compression, MmapBackend, TileCoord, TileType};

    fn make_mbtiles(path: &std::path::Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE metadata (name text, value text);
             CREATE TABLE tiles (zoom_level integer, tile_column integer, tile_row integer, tile_data blob);
             CREATE UNIQUE INDEX tile_index ON tiles (zoom_level, tile_column, tile_row);",
        )
        .unwrap();
        for (name, value) in [
            ("format", "png"),
            ("name", "test-archive"),
            ("bounds", "-180,-85,180,85"),
            ("minzoom", "0"),
            ("maxzoom", "1"),
        ] {
            conn.execute("INSERT INTO metadata VALUES (?1, ?2)", params![name, value])
                .unwrap();
        }
        // TMS rows: (1,0,0) is the southern tile, (1,0,1) the northern one.
        for (z, x, row, data) in [
            (0i64, 0i64, 0i64, &b"root"[..]),
            (1, 0, 0, &b"sw"[..]),
            (1, 0, 1, &b"nw"[..]),
        ] {
            conn.execute(
                "INSERT INTO tiles VALUES (?1, ?2, ?3, ?4)",
                params![z, x, row, data],
            )
            .unwrap();
        }
    }

    #[tokio::test]
    async fn mbtiles_roundtrip_flips_y_and_carries_metadata() {
        let tmp = TempDir::new().unwrap();
        let mbtiles = tmp.path().join("in.mbtiles");
        make_mbtiles(&mbtiles);

        let out = tmp.path().join("out.pmtiles");
        let stats = mbtiles_to_pmtiles(&mbtiles, &out).unwrap();
        assert_eq!(stats.tiles_read, 3);
        assert_eq!(stats.tiles_written, 3);
        assert_eq!(stats.min_zoom, Some(0));
        assert_eq!(stats.max_zoom, Some(1));

        let backend = MmapBackend::try_from(out.to_str().unwrap()).await.unwrap();
        let reader = AsyncPmTilesReader::try_from_source(backend).await.unwrap();
        let header = reader.get_header();
        assert_eq!(header.tile_type, TileType::Png);
        assert_eq!(header.tile_compression, Compression::None);
        assert_eq!(header.min_zoom, 0);
        assert_eq!(header.max_zoom, 1);

        // MBTiles TMS (1,0,0) -> PMTiles XYZ (1,0,1).
        let t = reader
            .get_tile(TileCoord::new(1, 0, 1).unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&*t, b"sw");
        let t = reader
            .get_tile(TileCoord::new(1, 0, 0).unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&*t, b"nw");

        let metadata = reader.get_metadata().await.unwrap();
        assert!(metadata.contains("test-archive"));
        // Structural keys must not leak into the JSON blob.
        assert!(!metadata.contains("minzoom"));
    }

    /// The de-duplicated `map` + `images` + `tiles` VIEW schema emitted by
    /// tippecanoe / mbutil must convert just like the flat table schema.
    #[tokio::test]
    async fn mbtiles_view_schema_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let mbtiles = tmp.path().join("view.mbtiles");
        let conn = Connection::open(&mbtiles).unwrap();
        conn.execute_batch(
            "CREATE TABLE metadata (name text, value text);
             CREATE TABLE map (zoom_level integer, tile_column integer, tile_row integer, tile_id text);
             CREATE TABLE images (tile_data blob, tile_id text);
             CREATE VIEW tiles AS
                 SELECT map.zoom_level, map.tile_column, map.tile_row, images.tile_data
                 FROM map JOIN images ON images.tile_id = map.tile_id;",
        )
        .unwrap();
        conn.execute("INSERT INTO metadata VALUES ('format', 'png')", [])
            .unwrap();
        // Two map entries share one image (content de-duplication).
        conn.execute(
            "INSERT INTO images VALUES (?1, 'a')",
            params![&b"shared"[..]],
        )
        .unwrap();
        conn.execute("INSERT INTO map VALUES (1, 0, 0, 'a')", [])
            .unwrap();
        conn.execute("INSERT INTO map VALUES (1, 1, 0, 'a')", [])
            .unwrap();

        let out = tmp.path().join("out.pmtiles");
        let stats = mbtiles_to_pmtiles(&mbtiles, &out).unwrap();
        assert_eq!(stats.tiles_read, 2);
        assert_eq!(stats.tiles_written, 2);

        let backend = MmapBackend::try_from(out.to_str().unwrap()).await.unwrap();
        let reader = AsyncPmTilesReader::try_from_source(backend).await.unwrap();
        // TMS (1,0,0) -> XYZ (1,0,1); TMS (1,1,0) -> XYZ (1,1,1).
        for (x, y) in [(0, 1), (1, 1)] {
            let t = reader
                .get_tile(TileCoord::new(1, x, y).unwrap())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(&*t, b"shared");
        }
    }
}
