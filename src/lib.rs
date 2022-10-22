// TODO: delete this!!!
// TODO: delete this!!!
// TODO: delete this!!!
#![allow(dead_code)]

use hilbert_2d::Variant;

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

fn tile_id(z: u8, x: u64, y: u64) -> u64 {
    if z == 0 {
        return 0;
    }

    let base_id: u64 = 1 + (1..z).map(|i| 4u64.pow(i as u32)).sum::<u64>();

    let tile_id =
        hilbert_2d::xy2h_discrete(x as usize, y as usize, z as usize, Variant::Hilbert) as u64;

    base_id + tile_id
}

pub struct Tile {
    data: Vec<u8>,
    tile_type: TileType,
    tile_compression: Compression,
}

#[cfg(test)]
mod test {
    use super::tile_id;

    #[test]
    fn test_tile_id() {
        assert_eq!(tile_id(0, 0, 0), 0);
        assert_eq!(tile_id(1, 1, 0), 4);
        assert_eq!(tile_id(2, 1, 3), 11);
        assert_eq!(tile_id(3, 3, 0), 26);
    }
}
