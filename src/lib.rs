#![forbid(unsafe_code)]

pub use crate::header::{Compression, Header, TileType};

mod directory;

mod error;
pub use error::PmtError;
#[cfg(feature = "http-async")]
pub use error::PmtHttpError;

mod header;

#[cfg(feature = "http-async")]
pub mod http;

#[cfg(feature = "mmap-async-tokio")]
pub mod mmap;

#[cfg(any(feature = "http-async", feature = "mmap-async-tokio"))]
pub mod async_reader;
pub mod tile;

#[cfg(test)]
mod tests {
    pub const RASTER_FILE: &str = "fixtures/stamen_toner(raster)CC-BY+ODbL_z3.pmtiles";
    pub const VECTOR_FILE: &str = "fixtures/protomaps(vector)ODbL_firenze.pmtiles";
}
