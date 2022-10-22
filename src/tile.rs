use crate::{Compression, TileType};
use bytes::Bytes;
use hilbert_2d::Variant;

pub(crate) fn tile_id(z: u8, x: u64, y: u64) -> u64 {
    if z == 0 {
        return 0;
    }

    let base_id: u64 = 1 + (1..z).map(|i| 4u64.pow(i as u32)).sum::<u64>();

    let tile_id =
        hilbert_2d::xy2h_discrete(x as usize, y as usize, z as usize, Variant::Hilbert) as u64;

    base_id + tile_id
}

pub struct Tile {
    pub data: Bytes,
    pub tile_type: TileType,
    pub tile_compression: Compression,
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
