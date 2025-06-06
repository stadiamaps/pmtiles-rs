use fast_hilbert::{h2xy, xy2h};

/// The pre-computed sizes of the tile pyramid for each zoom level.
/// The size at zoom level `z` (array index) is equal to the number of tiles before that zoom level.
///
/// ```
/// # use pmtiles::PYRAMID_SIZE_BY_ZOOM;
/// let mut size_at_level = 0_u64;
/// for z in 0..PYRAMID_SIZE_BY_ZOOM.len() {
///     assert_eq!(PYRAMID_SIZE_BY_ZOOM[z], size_at_level, "Invalid value at zoom {z}");
///     // add number of tiles at this zoom level
///     size_at_level += 4_u64.pow(z as u32);
/// }
/// ```
#[expect(clippy::unreadable_literal)]
pub const PYRAMID_SIZE_BY_ZOOM: [u64; 32] = [
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
];

/// Maximum valid Tile Zoom level in the `PMTiles` format.
///
/// ```
/// # use pmtiles::MAX_ZOOM;
/// assert_eq!(MAX_ZOOM, 31);
/// ```
#[expect(clippy::cast_possible_truncation)]
pub const MAX_ZOOM: u8 = PYRAMID_SIZE_BY_ZOOM.len() as u8 - 1;

/// Maximum valid Tile ID in the `PMTiles` format.
///
/// ```
/// # use pmtiles::MAX_TILE_ID;
/// assert_eq!(MAX_TILE_ID, 6148914691236517204);
/// ```
pub const MAX_TILE_ID: u64 =
    PYRAMID_SIZE_BY_ZOOM[PYRAMID_SIZE_BY_ZOOM.len() - 1] + 4_u64.pow(MAX_ZOOM as u32) - 1;

/// Represents a tile coordinate in the `PMTiles` format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TileCoord {
    z: u8,
    x: u32,
    y: u32,
}

impl TileCoord {
    /// Create a new coordinate with the given zoom level and tile coordinates, or return `None` if the values are invalid.
    ///
    /// ```
    /// # use pmtiles::TileCoord;
    /// let coord = TileCoord::new(18, 235085, 122323).unwrap();
    /// assert_eq!(coord.z(), 18);
    /// assert_eq!(coord.x(), 235085);
    /// assert_eq!(coord.y(), 122323);
    /// assert!(TileCoord::new(32, 1, 3).is_none()); // Invalid zoom level
    /// assert!(TileCoord::new(2, 4, 0).is_none()); // Invalid x coordinate
    /// assert!(TileCoord::new(2, 0, 4).is_none()); // Invalid y coordinate
    /// ```
    #[must_use]
    pub fn new(z: u8, x: u32, y: u32) -> Option<Self> {
        if z > MAX_ZOOM || x >= (1 << z) || y >= (1 << z) {
            return None;
        }
        Some(Self { z, x, y })
    }

    /// Get the zoom level of this coordinate.
    #[must_use]
    pub fn z(&self) -> u8 {
        self.z
    }

    /// Get the x coordinate of this tile.
    #[must_use]
    pub fn x(&self) -> u32 {
        self.x
    }

    /// Get the y coordinate of this tile.
    #[must_use]
    pub fn y(&self) -> u32 {
        self.y
    }
}

/// Represents a unique identifier for a tile in the `PMTiles` format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct TileId(u64);

impl TileId {
    /// Create a new `TileId` from the u64 value, or return `None` if the value is invalid.
    ///
    /// ```
    /// # use pmtiles::TileId;
    /// assert_eq!(TileId::new(0).unwrap().value(), 0);
    /// assert!(TileId::new(6148914691236517204).is_some());
    /// assert!(TileId::new(6148914691236517205).is_none());
    /// ```
    #[must_use]
    pub fn new(id: u64) -> Option<Self> {
        if id <= MAX_TILE_ID {
            Some(Self(id))
        } else {
            None
        }
    }

    /// Get the underlying u64 value of this `TileId`.
    #[must_use]
    pub fn value(self) -> u64 {
        self.0
    }
}

impl From<TileId> for u64 {
    fn from(tile_id: TileId) -> Self {
        tile_id.0
    }
}

impl From<TileId> for TileCoord {
    #[expect(clippy::cast_possible_truncation)]
    fn from(id: TileId) -> Self {
        let id = id.value();
        let mut z = 0;
        let mut size = 0;
        for (idx, &val) in PYRAMID_SIZE_BY_ZOOM.iter().enumerate() {
            if id < val {
                // If we never break, it means the id is for the last zoom level.
                // The ID has been verified to be <= MAX_TILE_ID, so this is safe.
                break;
            }
            z = idx as u8;
            size = val;
        }

        if z > 0 {
            // Extract the Hilbert curve index and convert it to tile coordinates
            let (x, y) = h2xy::<u32>(id - size, z);
            TileCoord { z, x, y }
        } else {
            TileCoord { z: 0, x: 0, y: 0 }
        }
    }
}

impl From<TileCoord> for TileId {
    fn from(coord: TileCoord) -> Self {
        let TileCoord { z, x, y } = coord;
        if z == 0 {
            // The 0/0/0 case would fail xy2h_discrete()
            TileId(0)
        } else {
            let base = PYRAMID_SIZE_BY_ZOOM
                .get(usize::from(z))
                .expect("TileCoord should be valid"); // see TileCoord::new
            let tile_id = xy2h(x, y, z);

            TileId(base + tile_id)
        }
    }
}

#[cfg(test)]
pub(crate) mod test {
    use crate::{MAX_TILE_ID, PYRAMID_SIZE_BY_ZOOM, TileCoord, TileId};

    pub fn coord(z: u8, x: u32, y: u32) -> TileCoord {
        TileCoord::new(z, x, y).unwrap()
    }

    pub fn coord_to_id(z: u8, x: u32, y: u32) -> u64 {
        TileId::from(coord(z, x, y)).value()
    }

    pub fn id_to_coord(id: u64) -> (u8, u32, u32) {
        let coord = TileCoord::from(TileId::new(id).unwrap());
        (coord.z(), coord.x(), coord.y())
    }

    #[test]
    #[expect(clippy::unreadable_literal)]
    fn test_tile_id() {
        assert_eq!(TileId::new(0).unwrap().value(), 0);
        assert_eq!(TileId::new(MAX_TILE_ID + 1), None);
        assert_eq!(TileId::new(MAX_TILE_ID).unwrap().value(), MAX_TILE_ID);

        assert_eq!(coord_to_id(0, 0, 0), 0);
        assert_eq!(coord_to_id(1, 1, 0), 4);
        assert_eq!(coord_to_id(2, 1, 3), 11);
        assert_eq!(coord_to_id(3, 3, 0), 26);
        assert_eq!(coord_to_id(20, 0, 0), 366503875925);
        assert_eq!(coord_to_id(21, 0, 0), 1466015503701);
        assert_eq!(coord_to_id(22, 0, 0), 5864062014805);
        assert_eq!(coord_to_id(23, 0, 0), 23456248059221);
        assert_eq!(coord_to_id(24, 0, 0), 93824992236885);
        assert_eq!(coord_to_id(25, 0, 0), 375299968947541);
        assert_eq!(coord_to_id(26, 0, 0), 1501199875790165);
        assert_eq!(coord_to_id(27, 0, 0), 6004799503160661);
        assert_eq!(coord_to_id(28, 0, 0), 24019198012642645);
        assert_eq!(coord_to_id(31, 0, 0), 1537228672809129301);
        let max_v = (1 << 31) - 1;
        assert_eq!(coord_to_id(31, max_v, max_v), 4611686018427387903);
        assert_eq!(coord_to_id(31, 0, max_v), 3074457345618258602);
        assert_eq!(coord_to_id(31, max_v, 0), 6148914691236517204);
    }

    #[test]
    fn round_trip_ids() {
        const LAST_PYRAMID_IDX: usize = PYRAMID_SIZE_BY_ZOOM.len() - 1;
        for id in [
            0,
            1,
            2,
            3,
            4,
            5,
            6,
            PYRAMID_SIZE_BY_ZOOM[LAST_PYRAMID_IDX],
            PYRAMID_SIZE_BY_ZOOM[LAST_PYRAMID_IDX] - 1,
            PYRAMID_SIZE_BY_ZOOM[LAST_PYRAMID_IDX] + 1,
            MAX_TILE_ID - 1,
            MAX_TILE_ID,
        ] {
            test_id(id);
        }
        for id in 0..1000 {
            test_id(id);
        }
    }

    fn test_id(id: u64) {
        let id1 = TileId::new(id).unwrap();
        let coord1 = TileCoord::from(id1);
        let coord2 = TileCoord::new(coord1.z, coord1.x, coord1.y).unwrap();
        let id2 = TileId::from(coord2);
        assert_eq!(id, id2.value(), "Failed round-trip for id={id}");
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
            let (z2, x2, y2) = id_to_coord(coord_to_id(z, x, y));
            assert_eq!(
                (z, x, y),
                (z2, x2, y2),
                "Failed round-trip for z={z}, x={x}, y={y}",
            );
        }
    }
}
