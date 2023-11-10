use std::num::NonZeroU64;
use std::panic::catch_unwind;

use bytes::{Buf, Bytes};

use crate::error::Error;

#[cfg(any(feature = "http-async", feature = "mmap-async-tokio"))]
pub(crate) const MAX_INITIAL_BYTES: usize = 16_384;
#[cfg(any(feature = "http-async", feature = "mmap-async-tokio", test))]
pub(crate) const HEADER_SIZE: usize = 127;

#[allow(dead_code)]
pub struct Header {
    pub(crate) version: u8,
    pub(crate) root_offset: u64,
    pub(crate) root_length: u64,
    pub(crate) metadata_offset: u64,
    pub(crate) metadata_length: u64,
    pub(crate) leaf_offset: u64,
    pub(crate) leaf_length: u64,
    pub(crate) data_offset: u64,
    pub(crate) data_length: u64,
    pub(crate) n_addressed_tiles: Option<NonZeroU64>,
    pub(crate) n_tile_entries: Option<NonZeroU64>,
    pub(crate) n_tile_contents: Option<NonZeroU64>,
    pub(crate) clustered: bool,
    pub(crate) internal_compression: Compression,
    pub tile_compression: Compression,
    pub tile_type: TileType,
    pub min_zoom: u8,
    pub max_zoom: u8,
    pub min_longitude: f32,
    pub min_latitude: f32,
    pub max_longitude: f32,
    pub max_latitude: f32,
    pub center_zoom: u8,
    pub center_longitude: f32,
    pub center_latitude: f32,
}

#[derive(Debug, Eq, PartialEq, Copy, Clone)]
pub enum Compression {
    Unknown,
    None,
    Gzip,
    Brotli,
    Zstd,
}

impl Compression {
    pub fn content_encoding(&self) -> Option<&'static str> {
        Some(match self {
            Compression::Gzip => "gzip",
            Compression::Brotli => "br",
            _ => None?,
        })
    }
}

impl TryInto<Compression> for u8 {
    type Error = Error;

    fn try_into(self) -> Result<Compression, Self::Error> {
        match self {
            0 => Ok(Compression::Unknown),
            1 => Ok(Compression::None),
            2 => Ok(Compression::Gzip),
            3 => Ok(Compression::Brotli),
            4 => Ok(Compression::Zstd),
            _ => Err(Error::InvalidCompression),
        }
    }
}

#[cfg(feature = "tilejson")]
impl Header {
    pub fn get_tilejson(&self, sources: Vec<String>) -> tilejson::TileJSON {
        tilejson::tilejson! {
            tiles: sources,
            minzoom: self.min_zoom,
            maxzoom: self.max_zoom,
            bounds: self.get_bounds(),
            center: self.get_center(),
        }
    }

    pub fn get_bounds(&self) -> tilejson::Bounds {
        tilejson::Bounds::new(
            self.min_longitude as f64,
            self.min_latitude as f64,
            self.max_longitude as f64,
            self.max_latitude as f64,
        )
    }

    pub fn get_center(&self) -> tilejson::Center {
        tilejson::Center::new(
            self.center_longitude as f64,
            self.center_latitude as f64,
            self.center_zoom,
        )
    }
}

#[derive(Debug, Eq, PartialEq, Copy, Clone)]
pub enum TileType {
    Unknown,
    Mvt,
    Png,
    Jpeg,
    Webp,
}

impl TileType {
    pub fn content_type(&self) -> &'static str {
        match self {
            TileType::Mvt => "application/vnd.mapbox-vector-tile",
            TileType::Png => "image/png",
            TileType::Webp => "image/webp",
            TileType::Jpeg => "image/jpeg",
            TileType::Unknown => "application/octet-stream",
        }
    }
}

impl TryInto<TileType> for u8 {
    type Error = Error;

    fn try_into(self) -> Result<TileType, Self::Error> {
        match self {
            0 => Ok(TileType::Unknown),
            1 => Ok(TileType::Mvt),
            2 => Ok(TileType::Png),
            3 => Ok(TileType::Jpeg),
            4 => Ok(TileType::Webp),
            _ => Err(Error::InvalidTileType),
        }
    }
}

static V3_MAGIC: &str = "PMTiles";
static V2_MAGIC: &str = "PM";

impl Header {
    fn read_coordinate_part<B: Buf>(mut buf: B) -> f32 {
        buf.get_i32_le() as f32 / 10_000_000.
    }

    pub fn try_from_bytes(mut bytes: Bytes) -> Result<Self, Error> {
        let magic_bytes = bytes.split_to(V3_MAGIC.len());

        // Assert magic
        if magic_bytes != V3_MAGIC {
            return Err(if magic_bytes.starts_with(V2_MAGIC.as_bytes()) {
                Error::UnsupportedPmTilesVersion
            } else {
                Error::InvalidMagicNumber
            });
        }

        // Wrap the panics that are possible in `get_u*_le` calls. (Panic occurs if the buffer is exhausted.)
        catch_unwind(move || {
            Ok(Self {
                version: bytes.get_u8(),
                root_offset: bytes.get_u64_le(),
                root_length: bytes.get_u64_le(),
                metadata_offset: bytes.get_u64_le(),
                metadata_length: bytes.get_u64_le(),
                leaf_offset: bytes.get_u64_le(),
                leaf_length: bytes.get_u64_le(),
                data_offset: bytes.get_u64_le(),
                data_length: bytes.get_u64_le(),
                n_addressed_tiles: NonZeroU64::new(bytes.get_u64_le()),
                n_tile_entries: NonZeroU64::new(bytes.get_u64_le()),
                n_tile_contents: NonZeroU64::new(bytes.get_u64_le()),
                clustered: bytes.get_u8() == 1,
                internal_compression: bytes.get_u8().try_into()?,
                tile_compression: bytes.get_u8().try_into()?,
                tile_type: bytes.get_u8().try_into()?,
                min_zoom: bytes.get_u8(),
                max_zoom: bytes.get_u8(),
                min_longitude: Self::read_coordinate_part(&mut bytes),
                min_latitude: Self::read_coordinate_part(&mut bytes),
                max_longitude: Self::read_coordinate_part(&mut bytes),
                max_latitude: Self::read_coordinate_part(&mut bytes),
                center_zoom: bytes.get_u8(),
                center_longitude: Self::read_coordinate_part(&mut bytes),
                center_latitude: Self::read_coordinate_part(&mut bytes),
            })
        })
        .map_err(|_| Error::InvalidHeader)?
    }
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::io::Read;
    use std::num::NonZeroU64;

    use bytes::{Bytes, BytesMut};

    use crate::header::{Header, TileType, HEADER_SIZE};
    use crate::tests::{RASTER_FILE, VECTOR_FILE};

    #[test]
    fn read_header() {
        let mut test = File::open(RASTER_FILE).unwrap();
        let mut header_bytes = [0; HEADER_SIZE];
        test.read_exact(header_bytes.as_mut_slice()).unwrap();

        let header = Header::try_from_bytes(Bytes::copy_from_slice(&header_bytes)).unwrap();

        // TODO: should be 3, but currently the ascii char 3, assert_eq!(header.version, 3);
        assert_eq!(header.tile_type, TileType::Png);
        assert_eq!(header.n_addressed_tiles, NonZeroU64::new(85));
        assert_eq!(header.n_tile_entries, NonZeroU64::new(84));
        assert_eq!(header.n_tile_contents, NonZeroU64::new(80));
        assert_eq!(header.min_zoom, 0);
        assert_eq!(header.max_zoom, 3);
        assert_eq!(header.center_zoom, 0);
        assert_eq!(header.center_latitude, 0.0);
        assert_eq!(header.center_longitude, 0.0);
        assert_eq!(header.min_latitude, -85.0);
        assert_eq!(header.max_latitude, 85.0);
        assert_eq!(header.min_longitude, -180.0);
        assert_eq!(header.max_longitude, 180.0);
        assert!(header.clustered);
    }

    #[test]
    fn read_valid_mvt_header() {
        let mut test = File::open(VECTOR_FILE).unwrap();
        let mut header_bytes = BytesMut::zeroed(HEADER_SIZE);
        test.read_exact(header_bytes.as_mut()).unwrap();

        let header = Header::try_from_bytes(header_bytes.freeze()).unwrap();

        assert_eq!(header.version, 3);
        assert_eq!(header.tile_type, TileType::Mvt);
        assert_eq!(header.n_addressed_tiles, NonZeroU64::new(108));
        assert_eq!(header.n_tile_entries, NonZeroU64::new(108));
        assert_eq!(header.n_tile_contents, NonZeroU64::new(106));
        assert_eq!(header.min_zoom, 0);
        assert_eq!(header.max_zoom, 14);
        assert_eq!(header.center_zoom, 0);
        assert_eq!(header.center_latitude, 43.779778);
        assert_eq!(header.center_longitude, 11.241483);
        assert_eq!(header.min_latitude, 43.727013);
        assert_eq!(header.max_latitude, 43.832542);
        assert_eq!(header.min_longitude, 11.154026);
        assert_eq!(header.max_longitude, 11.328939);
        assert!(header.clustered);
    }

    #[test]
    #[cfg(feature = "tilejson")]
    fn get_tilejson_raster() {
        use tilejson::{Bounds, Center};

        let mut test = File::open(RASTER_FILE).unwrap();
        let mut header_bytes = BytesMut::zeroed(HEADER_SIZE);
        test.read_exact(header_bytes.as_mut()).unwrap();
        let header = Header::try_from_bytes(header_bytes.freeze()).unwrap();
        let tj = header.get_tilejson(Vec::new());

        assert_eq!(tj.center, Some(Center::default()));
        assert_eq!(tj.bounds, Some(Bounds::new(-180.0, -85.0, 180.0, 85.0)));
    }

    #[test]
    #[cfg(feature = "tilejson")]
    fn get_tilejson_vector() {
        use tilejson::{Bounds, Center};

        let mut test = File::open(VECTOR_FILE).unwrap();
        let mut header_bytes = BytesMut::zeroed(HEADER_SIZE);
        test.read_exact(header_bytes.as_mut()).unwrap();
        let header = Header::try_from_bytes(header_bytes.freeze()).unwrap();
        let tj = header.get_tilejson(Vec::new());

        assert_eq!(
            tj.center,
            Some(Center::new(11.241482734680176, 43.77977752685547, 0))
        );

        assert_eq!(
            tj.bounds,
            Some(Bounds::new(
                11.15402603149414,
                43.727012634277344,
                11.328939437866211,
                43.832542419433594
            ))
        );
    }
}
