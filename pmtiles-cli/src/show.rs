//! Show subcommand
//!
//! Inspect a local or remote `PMTiles` archive.

use clap::Parser;
use pmtiles::{AsyncPmTilesReader, HttpBackend, MmapBackend};
use reqwest::Client;

#[derive(Parser, Debug)]
#[command(about = "Inspect a local or remote archive")]
pub struct Args {
    /// Path to `PMTiles` archive (local file or HTTP URL)
    #[arg(value_name = "PATH")]
    path: String,
}

/// Format `TileType` as lowercase string matching go-pmtiles output
fn format_tile_type(tile_type: pmtiles::TileType) -> &'static str {
    match tile_type {
        pmtiles::TileType::Mvt => "mvt",
        pmtiles::TileType::Png => "png",
        pmtiles::TileType::Jpeg => "jpeg",
        pmtiles::TileType::Webp => "webp",
        pmtiles::TileType::Avif => "avif",
        pmtiles::TileType::Unknown => "unknown",
    }
}

/// Format Compression as lowercase string matching go-pmtiles output
fn format_compression(compression: pmtiles::Compression) -> &'static str {
    match compression {
        pmtiles::Compression::Gzip => "gzip",
        pmtiles::Compression::Brotli => "brotli",
        pmtiles::Compression::Zstd => "zstd",
        pmtiles::Compression::None => "none",
        pmtiles::Compression::Unknown => "unknown",
    }
}

pub async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    // Open archive (support both local files and HTTP URLs)
    if args.path.starts_with("http://") || args.path.starts_with("https://") {
        let client = Client::builder()
            .user_agent(format!("pmtiles-rs-cli/{}", env!("CARGO_PKG_VERSION")))
            .build()?;
        let backend = HttpBackend::try_from(client, args.path.as_str())?;
        let reader = AsyncPmTilesReader::try_from_source(backend).await?;
        print_archive_info(&reader).await?;
    } else {
        let backend = MmapBackend::try_from(args.path.as_str()).await?;
        let reader = AsyncPmTilesReader::try_from_source(backend).await?;
        print_archive_info(&reader).await?;
    }

    Ok(())
}

async fn print_archive_info<B: pmtiles::AsyncBackend + Send + Sync>(
    reader: &AsyncPmTilesReader<B>,
) -> Result<(), Box<dyn std::error::Error>> {
    let header = reader.get_header();

    // Print header information
    println!("pmtiles spec version: {}", header.spec_version());
    println!("tile type: {}", format_tile_type(header.tile_type));
    println!(
        "bounds: (long: {:.6}, lat: {:.6}) (long: {:.6}, lat: {:.6})",
        header.min_longitude, header.min_latitude, header.max_longitude, header.max_latitude
    );
    println!("min zoom: {}", header.min_zoom);
    println!("max zoom: {}", header.max_zoom);
    println!(
        "center: (long: {:.6}, lat: {:.6})",
        header.center_longitude, header.center_latitude
    );
    println!("center zoom: {}", header.center_zoom);

    // Print tile counts (if available in header)
    if let Some(n) = header.n_addressed_tiles() {
        println!("addressed tiles count: {n}");
    } else {
        println!("addressed tiles count: unknown");
    }
    if let Some(n) = header.n_tile_entries() {
        println!("tile entries count: {n}");
    } else {
        println!("tile entries count: unknown");
    }
    if let Some(n) = header.n_tile_contents() {
        println!("tile contents count: {n}");
    } else {
        println!("tile contents count: unknown");
    }

    println!("clustered: {}", header.clustered());
    println!(
        "internal compression: {}",
        format_compression(header.internal_compression())
    );
    println!(
        "tile compression: {}",
        format_compression(header.tile_compression)
    );

    // Print metadata
    if let Ok(metadata) = reader.get_metadata().await
        && !metadata.is_empty()
    {
        // Parse metadata as JSON and print key-value pairs
        if let Ok(serde_json::Value::Object(obj)) =
            serde_json::from_str::<serde_json::Value>(&metadata)
        {
            for (key, value) in obj {
                // Format value based on type
                let value_str = match value {
                    serde_json::Value::String(s) => s,
                    serde_json::Value::Object(_) | serde_json::Value::Array(_) => {
                        "<object...>".to_string()
                    }
                    _ => value.to_string(),
                };
                println!("{key} {value_str}");
            }
        } else {
            println!("Expected metadata as json object but got: {metadata}");
        }
    }

    Ok(())
}
