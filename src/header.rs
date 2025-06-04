use std::num::NonZeroU64;
use std::panic::catch_unwind;

use bytes::{Buf, Bytes};

use crate::error::{PmtError, PmtResult};

#[cfg(any(feature = "__async", feature = "write"))]
pub(crate) const MAX_INITIAL_BYTES: usize = 16_384;
#[cfg(any(test, feature = "__async", feature = "write"))]
pub(crate) const HEADER_SIZE: usize = 127;

#[allow(dead_code)]
#[derive(Debug)]
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

impl Header {
    #[cfg(feature = "write")]
    pub(crate) fn new(tile_compression: Compression, tile_type: TileType) -> Self {
        #[expect(clippy::excessive_precision)]
        Self {
            version: 3,
            root_offset: HEADER_SIZE as u64,
            root_length: 0,
            metadata_offset: MAX_INITIAL_BYTES as u64,
            metadata_length: 0,
            leaf_offset: 0,
            leaf_length: 0,
            data_offset: 0,
            data_length: 0,
            n_addressed_tiles: None,
            n_tile_entries: None,
            n_tile_contents: None,
            clustered: true,
            internal_compression: Compression::Gzip,
            tile_compression,
            tile_type,
            min_zoom: 0,
            max_zoom: 22,
            min_longitude: -180.0,
            min_latitude: -85.051_129,
            max_longitude: 180.0,
            max_latitude: 85.051_129,
            center_zoom: 0,
            center_longitude: 0.0,
            center_latitude: 0.0,
        }
    }
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
    #[must_use]
    pub fn content_encoding(self) -> Option<&'static str> {
        Some(match self {
            Compression::Gzip => "gzip",
            Compression::Brotli => "br",
            _ => None?,
        })
    }
}

impl TryInto<Compression> for u8 {
    type Error = PmtError;

    fn try_into(self) -> Result<Compression, Self::Error> {
        match self {
            0 => Ok(Compression::Unknown),
            1 => Ok(Compression::None),
            2 => Ok(Compression::Gzip),
            3 => Ok(Compression::Brotli),
            4 => Ok(Compression::Zstd),
            _ => Err(PmtError::InvalidCompression),
        }
    }
}

#[cfg(feature = "tilejson")]
impl Header {
    #[must_use]
    pub fn get_tilejson(&self, sources: Vec<String>) -> tilejson::TileJSON {
        tilejson::tilejson! {
            tiles: sources,
            minzoom: self.min_zoom,
            maxzoom: self.max_zoom,
            bounds: self.get_bounds(),
            center: self.get_center(),
        }
    }

    #[must_use]
    pub fn get_bounds(&self) -> tilejson::Bounds {
        tilejson::Bounds::new(
            f64::from(self.min_longitude),
            f64::from(self.min_latitude),
            f64::from(self.max_longitude),
            f64::from(self.max_latitude),
        )
    }

    #[must_use]
    pub fn get_center(&self) -> tilejson::Center {
        tilejson::Center::new(
            f64::from(self.center_longitude),
            f64::from(self.center_latitude),
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
    #[must_use]
    pub fn content_type(self) -> &'static str {
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
    type Error = PmtError;

    fn try_into(self) -> Result<TileType, Self::Error> {
        match self {
            0 => Ok(TileType::Unknown),
            1 => Ok(TileType::Mvt),
            2 => Ok(TileType::Png),
            3 => Ok(TileType::Jpeg),
            4 => Ok(TileType::Webp),
            _ => Err(PmtError::InvalidTileType),
        }
    }
}

static V3_MAGIC: &str = "PMTiles";
static V2_MAGIC: &str = "PM";

impl Header {
    #[allow(clippy::cast_precision_loss)]
    fn read_coordinate_part<B: Buf>(mut buf: B) -> f32 {
        // TODO: would it be more precise to do `((value as f64) / 10_000_000.) as f32` ?
        buf.get_i32_le() as f32 / 10_000_000.
    }

    pub fn try_from_bytes(mut bytes: Bytes) -> PmtResult<Self> {
        let magic_bytes = bytes.split_to(V3_MAGIC.len());

        // Assert magic
        if magic_bytes != V3_MAGIC {
            return Err(if magic_bytes.starts_with(V2_MAGIC.as_bytes()) {
                PmtError::UnsupportedPmTilesVersion
            } else {
                PmtError::InvalidMagicNumber
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
        .map_err(|_| PmtError::InvalidHeader)?
    }
}

#[cfg(feature = "write")]
impl crate::writer::WriteTo for Header {
    fn write_to<W: std::io::Write>(&self, writer: &mut W) -> std::io::Result<()> {
        use std::num::NonZero;

        // Write a magic number
        writer.write_all(V3_MAGIC.as_bytes())?;

        // Write header fields
        writer.write_all(&[self.version])?;
        writer.write_all(&self.root_offset.to_le_bytes())?;
        writer.write_all(&self.root_length.to_le_bytes())?;
        writer.write_all(&self.metadata_offset.to_le_bytes())?;
        writer.write_all(&self.metadata_length.to_le_bytes())?;
        writer.write_all(&self.leaf_offset.to_le_bytes())?;
        writer.write_all(&self.leaf_length.to_le_bytes())?;
        writer.write_all(&self.data_offset.to_le_bytes())?;
        writer.write_all(&self.data_length.to_le_bytes())?;
        writer.write_all(&self.n_addressed_tiles.map_or(0, NonZero::get).to_le_bytes())?;
        writer.write_all(&self.n_tile_entries.map_or(0, NonZero::get).to_le_bytes())?;
        writer.write_all(&self.n_tile_contents.map_or(0, NonZero::get).to_le_bytes())?;
        writer.write_all(&[u8::from(self.clustered)])?;
        writer.write_all(&[self.internal_compression as u8])?;
        writer.write_all(&[self.tile_compression as u8])?;
        writer.write_all(&[self.tile_type as u8])?;
        writer.write_all(&[self.min_zoom])?;
        writer.write_all(&[self.max_zoom])?;
        Self::write_coordinate_part(writer, self.min_longitude)?;
        Self::write_coordinate_part(writer, self.min_latitude)?;
        Self::write_coordinate_part(writer, self.max_longitude)?;
        Self::write_coordinate_part(writer, self.max_latitude)?;
        writer.write_all(&[self.center_zoom])?;
        Self::write_coordinate_part(writer, self.center_longitude)?;
        Self::write_coordinate_part(writer, self.center_latitude)?;

        Ok(())
    }
}

impl Header {
    #[cfg(feature = "write")]
    #[allow(clippy::cast_possible_truncation)]
    fn write_coordinate_part<W: std::io::Write>(writer: &mut W, value: f32) -> std::io::Result<()> {
        writer.write_all(&((value * 10_000_000.0) as i32).to_le_bytes())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unreadable_literal, clippy::float_cmp)]
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

    #[test]
    #[cfg(feature = "write")]
    fn write_header() {
        use crate::writer::WriteTo as _;

        let mut test = File::open(RASTER_FILE).unwrap();
        let mut header_bytes = [0; HEADER_SIZE];
        test.read_exact(header_bytes.as_mut_slice()).unwrap();
        let header = Header::try_from_bytes(Bytes::copy_from_slice(&header_bytes)).unwrap();

        let mut buf = vec![];
        header.write_to(&mut buf).unwrap();
        let out = Header::try_from_bytes(Bytes::from(buf)).unwrap();
        assert_eq!(header.version, out.version);
        assert_eq!(header.tile_type, out.tile_type);
        assert_eq!(header.n_addressed_tiles, out.n_addressed_tiles);
        assert_eq!(header.n_tile_entries, out.n_tile_entries);
        assert_eq!(header.n_tile_contents, out.n_tile_contents);
        assert_eq!(header.min_zoom, out.min_zoom);
        assert_eq!(header.max_zoom, out.max_zoom);
        assert_eq!(header.center_zoom, out.center_zoom);
        assert_eq!(header.center_latitude, out.center_latitude);
        assert_eq!(header.center_longitude, out.center_longitude);
        assert_eq!(header.min_latitude, out.min_latitude);
        assert_eq!(header.max_latitude, out.max_latitude);
        assert_eq!(header.min_longitude, out.min_longitude);
        assert_eq!(header.max_longitude, out.max_longitude);
        assert_eq!(header.clustered, out.clustered);
    }
}
