#![allow(clippy::unreadable_literal)]

const PYRAMID_SIZE_BY_ZOOM: [u64; 21] = [
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
];

fn base_id_for_zoom(z: u8) -> u64 {
    if z == 0 {
        return 0;
    }
    let z_ind = usize::from(z);
    if z_ind < PYRAMID_SIZE_BY_ZOOM.len() {
        PYRAMID_SIZE_BY_ZOOM[z_ind]
    } else {
        let last_ind = PYRAMID_SIZE_BY_ZOOM.len() - 1;
        PYRAMID_SIZE_BY_ZOOM[last_ind] + (last_ind..z_ind).map(|i| 1_u64 << (i << 1)).sum::<u64>()
    }
}

/// Compute the tile id for a given zoom level and tile coordinates.
#[cfg(any(feature = "__async", feature = "write"))]
#[must_use]
pub(crate) fn calc_tile_id(z: u8, x: u64, y: u64) -> u64 {
    // The 0/0/0 case is not needed for the base id computation, but it will fail hilbert_2d::u64::xy2h_discrete
    if z == 0 {
        return 0;
    }

    let tile_id = hilbert_2d::u64::xy2h_discrete(x, y, z.into(), hilbert_2d::Variant::Hilbert);

    base_id_for_zoom(z) + tile_id
}

#[must_use]
pub(crate) fn calc_tile_coords(tile_id: u64) -> (u8, u64, u64) {
    if tile_id == 0 {
        return (0, 0, 0);
    }

    // Find the zoom level by comparing against pyramid sizes
    let mut z = 0u8;
    for (zoom, &pyramid_size) in PYRAMID_SIZE_BY_ZOOM.iter().enumerate() {
        // zoom is in the range 0..=20
        if tile_id < pyramid_size {
            // the lowest value of pyramid_size is 0, so we know that zoom - 1 is at least 0
            // and we can safely convert it to u8
            z = u8::try_from(zoom - 1).expect("malformed zoom level");
            break;
        }
    }

    // If we didn't find it in the precomputed array, calculate for higher zoom levels
    if z == 0 && tile_id >= PYRAMID_SIZE_BY_ZOOM[PYRAMID_SIZE_BY_ZOOM.len() - 1] {
        let last_ind = PYRAMID_SIZE_BY_ZOOM.len() - 1;
        let mut current_pyramid_size = PYRAMID_SIZE_BY_ZOOM[last_ind];
        // last_ind is 20 here
        z = u8::try_from(last_ind).expect("malformed zoom level");

        while tile_id >= current_pyramid_size {
            let tiles_at_zoom = 1_u64 << (z << 1);
            if tile_id < current_pyramid_size + tiles_at_zoom {
                break;
            }
            current_pyramid_size += tiles_at_zoom;
            z += 1;
        }
    }

    // Extract the Hilbert curve index
    let hilbert_index = tile_id - base_id_for_zoom(z);

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
