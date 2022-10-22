use std::io::Cursor;
use std::num::NonZeroU64;
use std::panic::catch_unwind;

use bytes::Buf;

use crate::error::Error;
use crate::Error::{InvalidMagicNumber, UnsupportedPmTilesVersion};

pub(crate) struct Header {
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
    pub(crate) tile_compression: Compression,
    pub(crate) tile_type: TileType,
    pub(crate) min_zoom: u8,
    pub(crate) max_zoom: u8,
    pub(crate) min_longitude: f32,
    pub(crate) min_latitude: f32,
    pub(crate) max_longitude: f32,
    pub(crate) max_latitude: f32,
    pub(crate) center_zoom: u8,
    pub(crate) center_longitude: f32,
    pub(crate) center_latitude: f32,
}

#[derive(Debug, Eq, PartialEq, Copy, Clone)]
pub enum Compression {
    Unknown,
    None,
    Gzip,
    Brotli,
    Zstd,
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

#[derive(Debug, Eq, PartialEq, Copy, Clone)]
pub enum TileType {
    Unknown,
    Mvt,
    Png,
    Jpeg,
    Webp,
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

    pub fn try_from_bytes(raw_bytes: &[u8; 127]) -> Result<Self, Error> {
        let mut bytes = Cursor::new(&raw_bytes[V3_MAGIC.len()..]);

        // Assert magic
        if &raw_bytes[0..V3_MAGIC.len()] != V3_MAGIC.as_bytes() {
            return if &raw_bytes[0..V2_MAGIC.len()] == V2_MAGIC.as_bytes() {
                Err(UnsupportedPmTilesVersion)
            } else {
                Err(InvalidMagicNumber)
            };
        }

        // TODO: why would this panic?
        catch_unwind(move || {
            Ok(Self {
                version: (bytes.get_u8() as char)
                    .to_digit(10)
                    .ok_or(Error::InvalidHeader)? as u8,
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

    use crate::header::{Header, TileType};

    #[test]
    fn read_header() {
        let mut test =
            File::open("fixtures/stamen_toner_z3.pmtiles").expect("Unable to open test file.");
        let mut header_bytes = [0; 127];
        test.read_exact(header_bytes.as_mut_slice())
            .expect("Unable to read header.");

        let header = Header::try_from_bytes(&header_bytes).expect("Unable to decode header");

        assert_eq!(header.version, 3);
        assert_eq!(header.tile_type, TileType::Png);
        assert_eq!(header.n_addressed_tiles, Some(85));
        assert_eq!(header.n_tile_entries, Some(84));
        assert_eq!(header.n_tile_contents, Some(80));
        assert_eq!(header.min_zoom, 0);
        assert_eq!(header.max_zoom, 3);
        assert_eq!(header.center_zoom, 0);
        assert_eq!(header.center_latitude, 0.0);
        assert_eq!(header.center_longitude, 0.0);
        assert_eq!(header.min_latitude, -85.0);
        assert_eq!(header.max_latitude, 85.0);
        assert_eq!(header.min_longitude, -180.0);
        assert_eq!(header.max_longitude, 180.0);
        assert_eq!(header.clustered, true);
    }
}
