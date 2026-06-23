//! Convert a directory of `z/x/y.<ext>` tiles into a `PMTiles` archive.
//!
//! GDAL's `gdal2tiles.py` (mercator profile) writes tiles in the `TMS` scheme by
//! default, so this example uses [`TileScheme::Tms`]. Pass `xyz` as the optional
//! 4th argument for "slippy"/Google tiles (e.g. `gdal2tiles --xyz`).
//!
//! ```sh
//! cargo run --example convert_tiledir --features tile-convert -- ./tiles out.pmtiles webp
//! ```

use pmtiles::{TileScheme, TileType, tile_dir_to_pmtiles};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let (Some(dir), Some(dst)) = (args.next(), args.next()) else {
        eprintln!(
            "usage: convert_tiledir <tile_dir> <output.pmtiles> [png|jpeg|webp|mvt] [tms|xyz]"
        );
        std::process::exit(2);
    };

    let tile_type = match args.next().as_deref() {
        Some("png") => TileType::Png,
        Some("jpeg" | "jpg") => TileType::Jpeg,
        Some("mvt" | "pbf") => TileType::Mvt,
        None | Some("webp") => TileType::Webp,
        Some(other) => {
            eprintln!("unknown tile type: {other}");
            std::process::exit(2);
        }
    };
    let scheme = match args.next().as_deref() {
        Some("xyz") => TileScheme::Xyz,
        None | Some("tms") => TileScheme::Tms,
        Some(other) => {
            eprintln!("unknown scheme: {other} (expected tms or xyz)");
            std::process::exit(2);
        }
    };

    let stats = tile_dir_to_pmtiles(&dir, &dst, tile_type, scheme)?;

    println!("Converted {dir} -> {dst}");
    println!("  tiles read:    {}", stats.tiles_read);
    println!("  tiles written: {}", stats.tiles_written);
    if let (Some(min), Some(max)) = (stats.min_zoom, stats.max_zoom) {
        println!("  zoom range:    {min}..={max}");
    }

    Ok(())
}
