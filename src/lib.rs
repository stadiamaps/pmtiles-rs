#![forbid(unsafe_code)]

#[cfg(feature = "__async")]
pub mod async_reader;
#[cfg(feature = "__async-aws-s3")]
mod backend_aws_s3;
#[cfg(feature = "http-async")]
mod backend_http;
#[cfg(feature = "mmap-async-tokio")]
mod backend_mmap;
#[cfg(feature = "__async-s3")]
mod backend_s3;
#[cfg(feature = "__async")]
pub mod cache;
mod directory;
mod error;
mod header;
#[cfg(feature = "__async")]
mod tile;
mod writer;

#[cfg(feature = "aws-s3-async")]
pub use backend_aws_s3::AwsS3Backend;
#[cfg(feature = "http-async")]
pub use backend_http::HttpBackend;
#[cfg(feature = "mmap-async-tokio")]
pub use backend_mmap::MmapBackend;
#[cfg(feature = "__async-s3")]
pub use backend_s3::S3Backend;
pub use directory::{DirEntry, Directory};
pub use error::{PmtError, PmtResult};
pub use header::{Compression, Header, TileType};
pub use writer::PmTilesWriter;
//
// Re-export crates exposed in our API to simplify dependency management
#[cfg(feature = "__async-aws-s3")]
pub use aws_sdk_s3;
#[cfg(feature = "http-async")]
pub use reqwest;
#[cfg(feature = "__async-s3")]
pub use s3;
#[cfg(feature = "tilejson")]
pub use tilejson;

#[cfg(test)]
mod tests {
    pub const RASTER_FILE: &str = "fixtures/stamen_toner(raster)CC-BY+ODbL_z3.pmtiles";
    pub const VECTOR_FILE: &str = "fixtures/protomaps(vector)ODbL_firenze.pmtiles";
}
