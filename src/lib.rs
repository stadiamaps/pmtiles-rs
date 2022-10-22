// TODO: delete this!!!
// TODO: delete this!!!
// TODO: delete this!!!
#![allow(dead_code)]

pub use crate::header::{Compression, Header, TileType};

mod directory;
pub mod error;
mod header;

#[cfg(feature = "http-async")]
pub mod http;

#[cfg(feature = "mmap-async-tokio")]
pub mod mmap;

#[cfg(feature = "tokio")]
pub mod async_reader;
pub mod tile;
