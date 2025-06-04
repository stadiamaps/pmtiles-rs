#![allow(clippy::unreadable_literal)]

/// The pre-computed sizes of the tile pyramid for each zoom level.
///
/// ```
/// # use crate::pmtiles::PYRAMID_SIZE_BY_ZOOM;
/// let mut size_at_level = 0_u64;
/// for z in 0..PYRAMID_SIZE_BY_ZOOM.len() {
///     assert_eq!(PYRAMID_SIZE_BY_ZOOM[z], size_at_level, "Invalid value at zoom {z}");
///     // add number of tiles at this zoom level
///     size_at_level += 4_u64.pow(z as u32);
/// }
/// ```
pub const PYRAMID_SIZE_BY_ZOOM: [u64; 33] = [
    /*  0 */ 0,
    /*  1 */ 1,
    /*  2 */ 5,
    /*  3 */ 21,
    /*  4 */ 85,
    /*  5 */ 341,
    /*  6 */ 1365,
    /*  7 */ 5461,
    /*  8 */ 21845,
    /*  9 */ 87381,
    /* 10 */ 349525,
    /* 11 */ 1398101,
    /* 12 */ 5592405,
    /* 13 */ 22369621,
    /* 14 */ 89478485,
    /* 15 */ 357913941,
    /* 16 */ 1431655765,
    /* 17 */ 5726623061,
    /* 18 */ 22906492245,
    /* 19 */ 91625968981,
    /* 20 */ 366503875925,
    /* 21 */ 1466015503701,
    /* 22 */ 5864062014805,
    /* 23 */ 23456248059221,
    /* 24 */ 93824992236885,
    /* 25 */ 375299968947541,
    /* 26 */ 1501199875790165,
    /* 27 */ 6004799503160661,
    /* 28 */ 24019198012642645,
    /* 29 */ 96076792050570581,
    /* 30 */ 384307168202282325,
    /* 31 */ 1537228672809129301,
    // this is the largest possible value because at z32 (base + 4^32) will overflow u64
    /* 32 */ 6148914691236517205,
];

/// Given a zoom level, get the base id for that zoom level.
/// The base id is the starting point for tile ids at that zoom level.
pub fn base_id_for_zoom(z: u8) -> Option<u64> {
    PYRAMID_SIZE_BY_ZOOM.get(usize::from(z)).copied()
}

/// Compute the tile id for a given zoom level and tile coordinates.
#[cfg(any(feature = "__async", feature = "write"))]
#[must_use]
pub(crate) fn calc_tile_id(z: u8, x: u64, y: u64) -> Option<u64> {
    // The 0/0/0 case is not needed for the base id computation, but it will fail hilbert_2d::u64::xy2h_discrete
    if z == 0 {
        return 0;
    }

    let tile_id = hilbert_2d::u64::xy2h_discrete(x, y, z.into(), hilbert_2d::Variant::Hilbert);

    base_id_for_zoom(z)? + tile_id
}

#[must_use]
pub(crate) fn calc_tile_coords(tile_id: u64) -> Option<(u8, u64, u64)> {
    if tile_id == 0 {
        return Some((0, 0, 0));
    }

    // Find the zoom level by comparing against pyramid sizes
    let (z, base) = PYRAMID_SIZE_BY_ZOOM.iter().enumerate().find(|(_, &size)| 
        tile_id <= size
    )?;

    for (zoom, &pyramid_size) in PYRAMID_SIZE_BY_ZOOM.iter().enumerate() {
        if tile_id <= pyramid_size {
            z = zoom as u8;
            base = pyramid_size;
            break;
        }
    }

    // Extract the Hilbert curve index
    let hilbert_index = tile_id - base_id_for_zoom(z).unwrap();

    // Convert back to x, y coordinates using inverse Hilbert curve
    let (x, y) =
        hilbert_2d::u64::h2xy_discrete(hilbert_index, z.into(), hilbert_2d::Variant::Hilbert);

    (z, x, y)
}

#[cfg(all(test, any(feature = "__async", feature = "write")))]
mod test {
    use super::{calc_tile_coords, calc_tile_id};

    #[test]
    fn test_calc_tile_id() {
        assert_eq!(calc_tile_id(0, 0, 0), 0);
        assert_eq!(calc_tile_id(1, 1, 0), 4);
        assert_eq!(calc_tile_id(2, 1, 3), 11);
        assert_eq!(calc_tile_id(3, 3, 0), 26);
        assert_eq!(calc_tile_id(20, 0, 0), 366503875925);
        assert_eq!(calc_tile_id(21, 0, 0), 1466015503701);
        assert_eq!(calc_tile_id(22, 0, 0), 5864062014805);
        assert_eq!(calc_tile_id(23, 0, 0), 23456248059221);
        assert_eq!(calc_tile_id(24, 0, 0), 93824992236885);
        assert_eq!(calc_tile_id(25, 0, 0), 375299968947541);
        assert_eq!(calc_tile_id(26, 0, 0), 1501199875790165);
        assert_eq!(calc_tile_id(27, 0, 0), 6004799503160661);
        assert_eq!(calc_tile_id(28, 0, 0), 24019198012642645);
    }

    #[test]
    fn test_calc_tile_coords() {
        // Test round-trip conversion
        let test_cases = [
            (0, 0, 0),
            (1, 1, 0),
            (2, 1, 3),
            (3, 3, 0),
            (20, 0, 0),
            (21, 0, 0),
            (22, 0, 0),
            (23, 0, 0),
            (24, 0, 0),
            (25, 0, 0),
            (26, 0, 0),
            (27, 0, 0),
            (28, 0, 0),
        ];

        for (z, x, y) in test_cases {
            let id = calc_tile_id(z, x, y);
            let (z_back, x_back, y_back) = calc_tile_coords(id);
            assert_eq!(
                (z, x, y),
                (z_back, x_back, y_back),
                "Failed round-trip for z={z}, x={x}, y={y}",
            );
        }
    }
}
