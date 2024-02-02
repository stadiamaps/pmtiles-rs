pub(crate) fn tile_id(z: u8, x: u64, y: u64) -> u64 {
    if z == 0 {
        return 0;
    }

    // TODO: minor optimization with bit shifting
    let base_id: u64 = 1 + (1..z).map(|i| 4u64.pow(u32::from(i))).sum::<u64>();

    let tile_id = hilbert_2d::u64::xy2h_discrete(x, y, z.into(), hilbert_2d::Variant::Hilbert);

    base_id + tile_id
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
