#[cfg(feature = "write")]
use std::io::{self, Write};
use std::num::NonZeroU64;

use bytes::{Buf, Bytes};
#[cfg(feature = "tilejson")]
use tilejson::{Bounds, Center, TileJSON};

#[cfg(feature = "write")]
use crate::writer::WriteTo;
use crate::{PmtError, PmtResult};

#[cfg(any(feature = "__async", feature = "write"))]
pub(crate) const MAX_INITIAL_BYTES: usize = 16_384;
pub(crate) const HEADER_SIZE: usize = 127;

#[derive(Debug)]
#[allow(dead_code)]
/// The header of a `PMTiles` file, containing metadata about the tiles.
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
    /// The compression used for tile data.
    pub tile_compression: Compression,
    /// The type of tiles.
    pub tile_type: TileType,
    /// The minimum zoom level.
    pub min_zoom: u8,
    /// The maximum zoom level.
    pub max_zoom: u8,
    /// The minimum longitude.
    pub min_longitude: f64,
    /// The minimum latitude.
    pub min_latitude: f64,
    /// The maximum longitude.
    pub max_longitude: f64,
    /// The maximum latitude.
    pub max_latitude: f64,
    /// The zoom level for the center point.
    pub center_zoom: u8,
    /// The longitude of the center point.
    pub center_longitude: f64,
    /// The latitude of the center point.
    pub center_latitude: f64,
}

impl Header {
    #[cfg(feature = "write")]
    pub(crate) fn new(tile_compression: Compression, tile_type: TileType) -> Self {
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

    /// Get the `PMTiles` spec version of the archive.
    #[must_use]
    pub fn spec_version(&self) -> u8 {
        self.version
    }

    /// Get the number of addressed tiles (sum of all run lengths)
    #[must_use]
    pub fn n_addressed_tiles(&self) -> Option<NonZeroU64> {
        self.n_addressed_tiles
    }

    /// Get the number of tile entries in the directory
    #[must_use]
    pub fn n_tile_entries(&self) -> Option<NonZeroU64> {
        self.n_tile_entries
    }

    /// Get the number of unique tile contents
    #[must_use]
    pub fn n_tile_contents(&self) -> Option<NonZeroU64> {
        self.n_tile_contents
    }

    /// Check if the archive is clustered (tiles stored in Hilbert curve order)
    #[must_use]
    pub fn clustered(&self) -> bool {
        self.clustered
    }

    /// Get the internal compression used for directories
    #[must_use]
    pub fn internal_compression(&self) -> Compression {
        self.internal_compression
    }
}

#[derive(Debug, Eq, PartialEq, Copy, Clone)]
/// Supported compression types for `PMTiles` data.
pub enum Compression {
    /// Unknown compression.
    Unknown,
    /// No compression.
    None,
    /// Gzip compression.
    Gzip,
    /// Brotli compression.
    Brotli,
    /// Zstandard compression.
    Zstd,
}

impl Compression {
    #[must_use]
    /// Returns the content encoding string for this compression type, if applicable.
    pub fn content_encoding(self) -> Option<&'static str> {
        Some(match self {
            Self::Gzip => "gzip",
            Self::Brotli => "br",
            Self::Zstd => "zstd",
            Self::Unknown | Self::None => None?,
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
    /// Generates a `TileJSON` object from the header data.
    pub fn get_tilejson(&self, sources: Vec<String>) -> TileJSON {
        tilejson::tilejson! {
            tiles: sources,
            minzoom: self.min_zoom,
            maxzoom: self.max_zoom,
            bounds: self.get_bounds(),
            center: self.get_center(),
        }
    }

    #[must_use]
    /// Returns the bounds of the tiles as a `TileJSON` Bounds object.
    pub fn get_bounds(&self) -> Bounds {
        Bounds::new(
            self.min_longitude,
            self.min_latitude,
            self.max_longitude,
            self.max_latitude,
        )
    }

    #[must_use]
    /// Returns the center point of the tiles as a `TileJSON` Center object.
    pub fn get_center(&self) -> Center {
        Center::new(
            self.center_longitude,
            self.center_latitude,
            self.center_zoom,
        )
    }
}

#[derive(Debug, Eq, PartialEq, Copy, Clone)]
/// Supported tile types for `PMTiles`.
pub enum TileType {
    /// Unknown tile type.
    Unknown,
    /// Mapbox Vector Tile.
    Mvt,
    /// PNG image tile.
    Png,
    /// JPEG image tile.
    Jpeg,
    /// WebP image tile.
    Webp,
    /// AVIF image tile.
    Avif,
    /// `MapLibre Vector Tile`.
    Mlt,
}

impl TileType {
    #[must_use]
    /// Returns the MIME content type for this tile type.
    pub fn content_type(self) -> &'static str {
        match self {
            Self::Mvt => "application/vnd.mapbox-vector-tile",
            Self::Png => "image/png",
            Self::Webp => "image/webp",
            Self::Jpeg => "image/jpeg",
            Self::Avif => "image/avif",
            Self::Mlt => "application/vnd.maplibre-vector-tile",
            Self::Unknown => "application/octet-stream",
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
            5 => Ok(TileType::Avif),
            6 => Ok(TileType::Mlt),
            _ => Err(PmtError::InvalidTileType),
        }
    }
}

static V3_MAGIC: &str = "PMTiles";
static V2_MAGIC: &str = "PM";

impl Header {
    fn read_coordinate_part<B: Buf>(mut buf: B) -> f64 {
        f64::from(buf.get_i32_le()) / 10_000_000.
    }

    /// Attempts to parse a Header from a byte buffer.
    ///
    /// # Errors
    ///
    /// If the byte buffer is shorter than a header, or contains invalid `PMTiles` header data.
    pub fn try_from_bytes(mut bytes: Bytes) -> PmtResult<Self> {
        // The magic number identifies the version, whatever the length of the data
        if !bytes.starts_with(V3_MAGIC.as_bytes()) {
            return Err(if V3_MAGIC.as_bytes().starts_with(&bytes) {
                // A truncated v3 header
                PmtError::InvalidHeader
            } else if bytes.starts_with(V2_MAGIC.as_bytes()) {
                PmtError::UnsupportedPmTilesVersion
            } else {
                PmtError::InvalidMagicNumber
            });
        }

        // Every read below then has enough bytes
        if bytes.len() < HEADER_SIZE {
            return Err(PmtError::InvalidHeader);
        }

        bytes.advance(V3_MAGIC.len()); // "PMTiles" = 7

        Ok(Self {
            version: bytes.get_u8(),                                  // 8
            root_offset: bytes.get_u64_le(),                          // 16
            root_length: bytes.get_u64_le(),                          // 24
            metadata_offset: bytes.get_u64_le(),                      // 32
            metadata_length: bytes.get_u64_le(),                      // 40
            leaf_offset: bytes.get_u64_le(),                          // 48
            leaf_length: bytes.get_u64_le(),                          // 56
            data_offset: bytes.get_u64_le(),                          // 64
            data_length: bytes.get_u64_le(),                          // 72
            n_addressed_tiles: NonZeroU64::new(bytes.get_u64_le()),   // 80
            n_tile_entries: NonZeroU64::new(bytes.get_u64_le()),      // 88
            n_tile_contents: NonZeroU64::new(bytes.get_u64_le()),     // 96
            clustered: bytes.get_u8() == 1,                           // 97
            internal_compression: bytes.get_u8().try_into()?,         // 98
            tile_compression: bytes.get_u8().try_into()?,             // 99
            tile_type: bytes.get_u8().try_into()?,                    // 100
            min_zoom: bytes.get_u8(),                                 // 101
            max_zoom: bytes.get_u8(),                                 // 102
            min_longitude: Self::read_coordinate_part(&mut bytes),    // 106
            min_latitude: Self::read_coordinate_part(&mut bytes),     // 110
            max_longitude: Self::read_coordinate_part(&mut bytes),    // 114
            max_latitude: Self::read_coordinate_part(&mut bytes),     // 118
            center_zoom: bytes.get_u8(),                              // 119
            center_longitude: Self::read_coordinate_part(&mut bytes), // 123
            center_latitude: Self::read_coordinate_part(&mut bytes),  // 127
        })
    }
}

#[cfg(feature = "write")]
impl WriteTo for Header {
    fn write_to<W: Write>(&self, writer: &mut W) -> io::Result<()> {
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
    #[expect(clippy::cast_possible_truncation)]
    fn write_coordinate_part<W: Write>(writer: &mut W, value: f64) -> io::Result<()> {
        writer.write_all(&((value * 10_000_000.0).round() as i32).to_le_bytes())
    }
}

#[cfg(test)]
mod tests {
    #![expect(clippy::unreadable_literal)]
    #![allow(
        clippy::float_cmp,
        reason = "exact round-trips; clippy versions differ on flagging float `assert_eq!`"
    )]

    use std::fs::{self, File};
    use std::io::Read as _;
    use std::num::NonZeroU64;

    use bytes::{Bytes, BytesMut};
    use rstest::rstest;

    use crate::header::HEADER_SIZE;
    use crate::tests::{RASTER_FILE, VECTOR_FILE};
    use crate::{Compression, Header, PmtError, PmtResult, TileType};

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
    fn header_getters() {
        let mut test = File::open(VECTOR_FILE).unwrap();
        let mut header_bytes = [0; HEADER_SIZE];
        test.read_exact(header_bytes.as_mut_slice()).unwrap();
        let header = Header::try_from_bytes(Bytes::copy_from_slice(&header_bytes)).unwrap();

        assert_eq!(header.spec_version(), 3);
        assert_eq!(header.n_addressed_tiles(), NonZeroU64::new(108));
        assert_eq!(header.n_tile_entries(), NonZeroU64::new(108));
        assert_eq!(header.n_tile_contents(), NonZeroU64::new(106));
        assert!(header.clustered());
        assert_eq!(header.internal_compression(), Compression::Gzip);
    }

    #[rstest]
    #[case(b"PMTiles", None)]
    #[case(b"PM\x02\x00\x00\x00\x00", Some(PmtError::UnsupportedPmTilesVersion))]
    #[case(b"NOTPMTI", Some(PmtError::InvalidMagicNumber))]
    fn header_magic_number(#[case] magic: &[u8], #[case] error: Option<PmtError>) {
        let mut bytes = fs::read(RASTER_FILE).unwrap();
        bytes[..magic.len()].copy_from_slice(magic);
        let result = Header::try_from_bytes(Bytes::from(bytes));
        match error {
            None => assert!(result.is_ok()),
            Some(error) => assert_eq!(result.unwrap_err().to_string(), error.to_string()),
        }
    }

    /// A truncated v3 header is an invalid header, not a panic.
    #[rstest]
    #[case::empty(0)]
    #[case::shorter_than_magic(3)]
    #[case::magic_only(7)]
    #[case::one_byte_short(HEADER_SIZE - 1)]
    fn header_rejects_short_data(#[case] len: usize) {
        let bytes = fs::read(RASTER_FILE).unwrap();
        let result = Header::try_from_bytes(Bytes::copy_from_slice(&bytes[..len]));
        assert!(matches!(result, Err(PmtError::InvalidHeader)));
    }

    /// Data shorter than a v3 header is still identified by its magic number.
    #[rstest]
    #[case::v2(b"PM\x02\x00", PmtError::UnsupportedPmTilesVersion)]
    #[case::not_pmtiles(b"GIF89a", PmtError::InvalidMagicNumber)]
    fn header_identifies_short_data(#[case] data: &'static [u8], #[case] error: PmtError) {
        let result = Header::try_from_bytes(Bytes::from_static(data));
        assert_eq!(result.unwrap_err().to_string(), error.to_string());
    }

    #[rstest]
    #[case(0, Some(Compression::Unknown), None)]
    #[case(1, Some(Compression::None), None)]
    #[case(2, Some(Compression::Gzip), Some("gzip"))]
    #[case(3, Some(Compression::Brotli), Some("br"))]
    #[case(4, Some(Compression::Zstd), Some("zstd"))]
    #[case(5, None, None)]
    fn compression_from_u8(
        #[case] value: u8,
        #[case] compression: Option<Compression>,
        #[case] encoding: Option<&str>,
    ) {
        let result: PmtResult<Compression> = value.try_into();
        assert_eq!(result.ok(), compression);
        if let Some(compression) = compression {
            assert_eq!(compression.content_encoding(), encoding);
        }
    }

    #[rstest]
    #[case(0, Some(TileType::Unknown), "application/octet-stream")]
    #[case(1, Some(TileType::Mvt), "application/vnd.mapbox-vector-tile")]
    #[case(2, Some(TileType::Png), "image/png")]
    #[case(3, Some(TileType::Jpeg), "image/jpeg")]
    #[case(4, Some(TileType::Webp), "image/webp")]
    #[case(5, Some(TileType::Avif), "image/avif")]
    #[case(6, Some(TileType::Mlt), "application/vnd.maplibre-vector-tile")]
    #[case(7, None, "")]
    fn tile_type_from_u8(
        #[case] value: u8,
        #[case] tile_type: Option<TileType>,
        #[case] content_type: &str,
    ) {
        let result: PmtResult<TileType> = value.try_into();
        assert_eq!(result.ok(), tile_type);
        if let Some(tile_type) = tile_type {
            assert_eq!(tile_type.content_type(), content_type);
        }
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
        assert_eq!(header.center_latitude, 43.779779);
        assert_eq!(header.center_longitude, 11.2414827);
        assert_eq!(header.min_latitude, 43.7270125);
        assert_eq!(header.max_latitude, 43.8325455);
        assert_eq!(header.min_longitude, 11.154026);
        assert_eq!(header.max_longitude, 11.3289395);
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

        assert_eq!(tj.center, Some(Center::new(11.2414827, 43.779779, 0)));

        assert_eq!(
            tj.bounds,
            Some(Bounds::new(11.154026, 43.7270125, 11.3289395, 43.8325455))
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
