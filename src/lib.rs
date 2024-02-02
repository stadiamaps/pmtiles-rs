#![forbid(unsafe_code)]

mod backend;
mod directory;
mod error;
mod header;
mod tile;

#[cfg(any(
    feature = "http-async",
    feature = "mmap-async-tokio",
    feature = "s3-async-rustls",
    feature = "s3-async-native"
))]
pub mod async_reader;

#[cfg(any(
    feature = "http-async",
    feature = "mmap-async-tokio",
    feature = "s3-async-native",
    feature = "s3-async-rustls"
))]
pub mod cache;

#[cfg(feature = "http-async")]
pub use backend::HttpBackend;
#[cfg(feature = "mmap-async-tokio")]
pub use backend::MmapBackend;
#[cfg(any(feature = "s3-async-rustls", feature = "s3-async-native"))]
pub use backend::S3Backend;
pub use directory::{DirEntry, Directory};
pub use error::{PmtError, PmtResult};
pub use header::{Compression, Header, TileType};
#[cfg(feature = "http-async")]
pub use reqwest;
#[cfg(any(feature = "s3-async-rustls", feature = "s3-async-native"))]
pub use s3;

#[cfg(test)]
mod tests {
    pub const RASTER_FILE: &str = "fixtures/stamen_toner(raster)CC-BY+ODbL_z3.pmtiles";
    pub const VECTOR_FILE: &str = "fixtures/protomaps(vector)ODbL_firenze.pmtiles";
}
