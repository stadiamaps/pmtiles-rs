#![cfg_attr(all(feature = "default"), doc = include_str!("../README.md"))]

#[cfg(feature = "__async")]
mod async_reader;
#[cfg(feature = "__async")]
pub use async_reader::{AsyncBackend, AsyncPmTilesReader};

pub mod backends;

#[doc(hidden)]
#[deprecated(since = "0.16.0", note = "Use `backends::aws_s3` instead")]
#[cfg(feature = "__async-aws-s3")]
pub use backends::aws_s3 as backend_aws_s3;
#[doc(hidden)]
#[deprecated(since = "0.16.0", note = "Use `backends::http` instead")]
#[cfg(feature = "http-async")]
pub use backends::http as backend_http;
#[doc(hidden)]
#[deprecated(since = "0.16.0", note = "Use `backends::mmap` instead")]
#[cfg(feature = "mmap-async-tokio")]
pub use backends::mmap as backend_mmap;
#[doc(hidden)]
#[deprecated(since = "0.16.0", note = "Use `backends::object_store` instead")]
#[cfg(feature = "object-store")]
pub use backends::object_store as backend_object_store;
#[doc(hidden)]
#[deprecated(since = "0.16.0", note = "Use `backends::s3` instead")]
#[cfg(feature = "__async-s3")]
pub use backends::s3 as backend_s3;

#[cfg(feature = "__async")]
mod cache;
#[cfg(feature = "__async")]
pub use cache::{DirCacheResult, DirectoryCache, HashMapCache, NoCache};

mod directory;
mod error;
mod header;
mod tile;
#[cfg(feature = "write")]
mod writer;

/// Re-export of crate exposed in our API to simplify dependency management
#[cfg(feature = "__async-aws-s3")]
pub use aws_sdk_s3;
#[cfg(feature = "aws-s3-async")]
pub use backends::aws_s3::AwsS3Backend;
#[cfg(feature = "http-async")]
pub use backends::http::HttpBackend;
#[cfg(feature = "mmap-async-tokio")]
pub use backends::mmap::MmapBackend;
#[cfg(feature = "object-store")]
pub use backends::object_store::ObjectStoreBackend;
#[cfg(feature = "__async-s3")]
pub use backends::s3::S3Backend;
#[cfg(feature = "iter-async")]
pub use directory::DirEntryCoordsIter;
pub use directory::{DirEntry, Directory};
pub use error::{PmtError, PmtResult};
pub use header::{Compression, Header, TileType};
/// Re-export of crate exposed in our API to simplify dependency management
#[cfg(feature = "http-async")]
pub use reqwest;
/// Re-export of crate exposed in our API to simplify dependency management
#[cfg(feature = "__async-s3")]
pub use s3;
pub use tile::{MAX_TILE_ID, MAX_ZOOM, PYRAMID_SIZE_BY_ZOOM, TileCoord, TileId};
/// Re-export of crate exposed in our API to simplify dependency management
#[cfg(feature = "tilejson")]
pub use tilejson;
#[cfg(feature = "write")]
pub use writer::{PmTilesStreamWriter, PmTilesWriter};

#[cfg(test)]
mod tests {
    pub const RASTER_FILE: &str = "fixtures/stamen_toner(raster)CC-BY+ODbL_z3.pmtiles";
    pub const VECTOR_FILE: &str = "fixtures/protomaps(vector)ODbL_firenze.pmtiles";
}
