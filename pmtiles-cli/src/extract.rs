//! Extract subcommand
//!
//! Extract a subset of tiles from a `PMTiles` archive based on a bounding box.

use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;
use std::time::Instant;

use clap::Parser;
use pmtiles::extract::{BoundingBox, Extractor};
use pmtiles::{AsyncBackend, AsyncPmTilesReader, HttpBackend, MmapBackend};
use reqwest::Client;

#[derive(Parser, Debug)]
#[command(about = "Extract a subset of tiles from a PMTiles archive")]
pub struct Args {
    /// Input `PMTiles` archive (local file or HTTP URL)
    #[arg(value_name = "INPUT")]
    input: String,

    /// Output `PMTiles` archive (local file)
    #[arg(value_name = "OUTPUT")]
    output: PathBuf,

    /// Bounding box in format: `min_lon,min_lat,max_lon,max_lat`
    ///
    /// Example: -122.5,37.7,-122.4,37.8 for San Francisco
    #[arg(long, value_name = "BBOX")]
    bbox: String,

    /// Minimum zoom level (inclusive). If not specified, uses archive's min zoom.
    #[arg(long, value_name = "ZOOM")]
    min_zoom: Option<u8>,

    /// Maximum zoom level (inclusive). If not specified, uses archive's max zoom.
    #[arg(long, value_name = "ZOOM")]
    max_zoom: Option<u8>,

    /// Overfetch ratio. Higher values download more unused data to reduce requests.
    #[arg(long, default_value = "0.05", value_name = "RATIO")]
    overfetch: f32,

    /// Number of concurrent requests for fetching data.
    #[arg(long, default_value = "4", value_name = "N")]
    concurrency: usize,
}

/// Print header information
fn print_header_info(header: &pmtiles::Header) {
    println!("Source archive:");
    println!("  Type:        {:?}", header.tile_type);
    println!("  Compression: {:?}", header.tile_compression);
    println!("  Zoom range:  {}-{}", header.min_zoom, header.max_zoom);
    println!(
        "  Bounds:      {},{},{},{}",
        header.min_longitude, header.min_latitude, header.max_longitude, header.max_latitude
    );
    println!();
}

/// Parse bbox string in format: `min_lon,min_lat,max_lon,max_lat`
fn parse_bbox(bbox: &str) -> Result<BoundingBox, String> {
    let parts: Vec<&str> = bbox.split(',').collect();
    if parts.len() != 4 {
        return Err(format!(
            "Invalid bbox format. Expected 'min_lon,min_lat,max_lon,max_lat', got '{bbox}'"
        ));
    }

    let min_lon = parts[0]
        .parse::<f64>()
        .map_err(|_| format!("Invalid min_lon: '{}'", parts[0]))?;
    let min_lat = parts[1]
        .parse::<f64>()
        .map_err(|_| format!("Invalid min_lat: '{}'", parts[1]))?;
    let max_lon = parts[2]
        .parse::<f64>()
        .map_err(|_| format!("Invalid max_lon: '{}'", parts[2]))?;
    let max_lat = parts[3]
        .parse::<f64>()
        .map_err(|_| format!("Invalid max_lat: '{}'", parts[3]))?;

    // Validate ranges
    if !(-180.0..=180.0).contains(&min_lon) {
        return Err(format!("min_lon out of range [-180, 180]: {min_lon}"));
    }
    if !(-180.0..=180.0).contains(&max_lon) {
        return Err(format!("max_lon out of range [-180, 180]: {max_lon}"));
    }
    if !(-90.0..=90.0).contains(&min_lat) {
        return Err(format!("min_lat out of range [-90, 90]: {min_lat}"));
    }
    if !(-90.0..=90.0).contains(&max_lat) {
        return Err(format!("max_lat out of range [-90, 90]: {max_lat}"));
    }
    if min_lon >= max_lon {
        return Err(format!(
            "min_lon must be less than max_lon: {min_lon} >= {max_lon}"
        ));
    }
    if min_lat >= max_lat {
        return Err(format!(
            "min_lat must be less than max_lat: {min_lat} >= {max_lat}"
        ));
    }

    Ok(BoundingBox::from_nesw(max_lat, max_lon, min_lat, min_lon))
}

pub async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    if args.input.starts_with("http://") || args.input.starts_with("https://") {
        println!("Opening remote archive...");
        let client = Client::builder()
            .user_agent("pmtiles-rs-cli/0.1.0")
            .build()?;
        let backend = HttpBackend::try_from(client, args.input.as_str())?;
        extract_with_backend(backend, args).await
    } else {
        println!("Opening local archive...");
        let backend = MmapBackend::try_from(args.input.as_str()).await?;
        extract_with_backend(backend, args).await
    }
}

pub async fn extract_with_backend(
    backend: impl AsyncBackend + Sync + Send,
    args: Args,
) -> Result<(), Box<dyn std::error::Error>> {
    let bbox = parse_bbox(&args.bbox).map_err(|e| format!("Error parsing bbox: {e}"))?;

    // Validate overfetch
    if args.overfetch < 0. {
        return Err(format!("overfetch cannot be negative, got {}", args.overfetch).into());
    }

    println!("PMTiles Extract");
    println!("===============");
    println!("Input:     {}", args.input);
    println!("Output:    {}", args.output.display());
    println!(
        "Bbox:      {},{},{},{}",
        bbox.min_lon, bbox.min_lat, bbox.max_lon, bbox.max_lat
    );
    if let Some(min_zoom) = args.min_zoom {
        println!("Min zoom:  {min_zoom}");
    }
    if let Some(max_zoom) = args.max_zoom {
        println!("Max zoom:  {max_zoom}");
    }
    println!("Overfetch: {:.1}%", args.overfetch * 100.0);
    println!();

    let start = Instant::now();

    let mut reader = AsyncPmTilesReader::try_from_source(backend).await?;

    let header = reader.get_header();
    print_header_info(header);

    println!("Extract Progress");
    println!("================");
    let mut output = BufWriter::new(File::create(&args.output)?);
    let mut extractor = Extractor::new(&mut reader)
        .overfetch(args.overfetch)
        .concurrency(args.concurrency)
        .progress(&|ratio| {
            print!("\r  ...reading index: {:>5.1}%", ratio * 100.0);
            std::io::Write::flush(&mut std::io::stdout()).ok();
        });
    if let Some(min_zoom) = args.min_zoom {
        extractor = extractor.min_zoom(min_zoom);
    }
    if let Some(max_zoom) = args.max_zoom {
        extractor = extractor.max_zoom(max_zoom);
    }

    let plan = extractor.prepare(bbox).await?;
    println!();
    extractor = extractor.progress(&|ratio| {
        print!("\r  ...fetching tiles: {:>5.1}%", ratio * 100.0);
        std::io::Write::flush(&mut std::io::stdout()).ok();
    });
    let stats = extractor.extract_to_writer(plan, &mut output).await?;
    println!();
    let elapsed = start.elapsed();

    println!();
    println!("Extract complete!");
    println!("===================");
    println!("Tiles extracted:        {}", stats.tile_contents());
    let tile_bytes_transferred = format_bytes(stats.total_tile_transfer_bytes());
    println!("Tile bytes transferred: {tile_bytes_transferred}");
    let tile_bytes_used = format_bytes(stats.tile_data_length());
    println!("Tile bytes used:        {tile_bytes_used}");

    let overfetch_bytes = stats.total_tile_transfer_bytes() - stats.tile_data_length();
    #[allow(clippy::cast_precision_loss)]
    let overfetch_pct = (overfetch_bytes as f64 / stats.tile_data_length() as f64) * 100.0;
    println!(
        "Overfetch:              {overfetch_bytes} ({overfetch_pct:.1}%)",
        overfetch_bytes = format_bytes(overfetch_bytes)
    );
    println!("Time:                   {:.2}s", elapsed.as_secs_f64());
    println!();
    println!("Output written to: {}", args.output.display());

    Ok(())
}

fn format_bytes(byte_count: u64) -> impl std::fmt::Display {
    bytesize::ByteSize(byte_count).display().si()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::float_cmp)]
    #[test]
    fn test_parse_bbox_valid() {
        let bbox = parse_bbox("-122.5,37.0,-122.0,37.5").unwrap();
        assert_eq!(bbox.min_lon, -122.5);
        assert_eq!(bbox.min_lat, 37.0);
        assert_eq!(bbox.max_lon, -122.0);
        assert_eq!(bbox.max_lat, 37.5);
    }

    #[test]
    fn test_parse_bbox_invalid_format() {
        assert!(parse_bbox("1,2,3").is_err());
        assert!(parse_bbox("1,2,3,4,5").is_err());
    }

    #[test]
    fn test_parse_bbox_invalid_numbers() {
        assert!(parse_bbox("a,2,3,4").is_err());
        assert!(parse_bbox("1,b,3,4").is_err());
    }

    #[test]
    fn test_parse_bbox_out_of_range() {
        assert!(parse_bbox("-181,0,0,0").is_err()); // min_lon too low
        assert!(parse_bbox("0,-91,0,0").is_err()); // min_lat too low
        assert!(parse_bbox("0,0,181,0").is_err()); // max_lon too high
        assert!(parse_bbox("0,0,0,91").is_err()); // max_lat too high
    }

    #[test]
    fn test_parse_bbox_inverted() {
        assert!(parse_bbox("10,0,5,0").is_err()); // min_lon >= max_lon
        assert!(parse_bbox("0,10,0,5").is_err()); // min_lat >= max_lat
    }
}
