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
    /// Inspect a local or remote archive
    Show(show::Args),
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logger from RUST_LOG environment variable
    // Example: RUST_LOG=debug pmtiles show ...
    env_logger::init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Show(args) => show::run(args).await,
    }
}
