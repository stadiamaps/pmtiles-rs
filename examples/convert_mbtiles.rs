//! Convert an `MBTiles` archive into a `PMTiles` archive.
//!
//! ```sh
//! cargo run --example convert_mbtiles --features mbtiles -- input.mbtiles output.pmtiles
//! ```

use pmtiles::mbtiles_to_pmtiles;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let (Some(src), Some(dst)) = (args.next(), args.next()) else {
        eprintln!("usage: convert_mbtiles <input.mbtiles> <output.pmtiles>");
        std::process::exit(2);
    };

    let stats = mbtiles_to_pmtiles(&src, &dst)?;

    println!("Converted {src} -> {dst}");
    println!("  tiles read:    {}", stats.tiles_read);
    println!("  tiles written: {}", stats.tiles_written);
    if let (Some(min), Some(max)) = (stats.min_zoom, stats.max_zoom) {
        println!("  zoom range:    {min}..={max}");
    }

    Ok(())
}
