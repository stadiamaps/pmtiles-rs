#![forbid(unsafe_code)]

pub use directory::{DirEntry, Directory};
#[cfg(feature = "http-async")]
pub use error::PmtHttpError;
pub use error::{PmtError, PmtResult};

pub use header::{Compression, Header, TileType};

#[cfg(any(feature = "s3-async-rustls", feature = "s3-async-native"))]
pub use backend::s3::S3Backend;

#[cfg(feature = "http-async")]
pub use backend::http::HttpBackend;

#[cfg(feature = "mmap-async-tokio")]
pub use backend::mmap::MmapBackend;

mod tile;

mod header;

mod directory;

mod error;

mod backend;

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

#[cfg(test)]
mod tests {
    pub const RASTER_FILE: &str = "fixtures/stamen_toner(raster)CC-BY+ODbL_z3.pmtiles";
    pub const VECTOR_FILE: &str = "fixtures/protomaps(vector)ODbL_firenze.pmtiles";
}
