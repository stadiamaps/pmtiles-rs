mod extract;
mod show;

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "pmtiles")]
#[command(about = "PMTiles CLI tool", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Extract a subset of tiles from a `PMTiles` archive
    Extract(extract::Args),
    /// Inspect a local or remote archive
    Show(show::Args),
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logger from RUST_LOG environment variable
    // Example: RUST_LOG=debug pmtiles extract ...
    env_logger::init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Extract(args) => extract::run(args).await,
        Commands::Show(args) => show::run(args).await,
    }
}
