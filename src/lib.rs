#![forbid(unsafe_code)]

mod tile;

mod header;
pub use crate::header::{Compression, Header, TileType};

mod directory;
pub use directory::{DirEntry, Directory};

mod error;
pub use error::PmtError;
#[cfg(feature = "http-async")]
pub use error::PmtHttpError;

#[cfg(feature = "http-async")]
pub mod http;

#[cfg(feature = "mmap-async-tokio")]
pub mod mmap;

#[cfg(any(feature = "http-async", feature = "mmap-async-tokio"))]
pub mod async_reader;

#[cfg(any(feature = "http-async", feature = "mmap-async-tokio"))]
pub mod cache;

#[cfg(test)]
mod tests {
    pub const RASTER_FILE: &str = "fixtures/stamen_toner(raster)CC-BY+ODbL_z3.pmtiles";
    pub const VECTOR_FILE: &str = "fixtures/protomaps(vector)ODbL_firenze.pmtiles";
}
