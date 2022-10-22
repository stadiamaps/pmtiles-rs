// TODO: delete this!!!
// TODO: delete this!!!
// TODO: delete this!!!
#![allow(dead_code)]

use directory::{Directory, Entry};

pub use crate::header::{Compression, Header, TileType};

mod directory;
pub mod error;
mod header;

#[cfg(feature = "http-async")]
pub mod http;

#[cfg(any(feature = "mmap-async-tokio", test))]
pub mod mmap;

// TODO: make an optional feature
pub mod async_reader;
pub mod tile;
