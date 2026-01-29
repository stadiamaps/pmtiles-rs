use std::num::NonZeroU64;
use std::panic::catch_unwind;

use bytes::{Buf, Bytes};

use crate::{PmtError, PmtResult};

#[cfg(any(feature = "__async", feature = "write"))]
pub(crate) const MAX_INITIAL_BYTES: usize = 16_384;
#[cfg(any(test, feature = "__async", feature = "write"))]
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
            Compression::Gzip => "gzip",
            Compression::Brotli => "br",
            Compression::Zstd => "zstd",
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
    /// Generates a `TileJSON` object from the header data.
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
    /// Returns the bounds of the tiles as a `TileJSON` Bounds object.
    pub fn get_bounds(&self) -> tilejson::Bounds {
        tilejson::Bounds::new(
            self.min_longitude,
            self.min_latitude,
            self.max_longitude,
            self.max_latitude,
        )
    }

    #[must_use]
    /// Returns the center point of the tiles as a `TileJSON` Center object.
    pub fn get_center(&self) -> tilejson::Center {
        tilejson::Center::new(
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
}

impl TileType {
    #[must_use]
    /// Returns the MIME content type for this tile type.
    pub fn content_type(self) -> &'static str {
        match self {
            TileType::Mvt => "application/vnd.mapbox-vector-tile",
            TileType::Png => "image/png",
            TileType::Webp => "image/webp",
            TileType::Jpeg => "image/jpeg",
            TileType::Avif => "image/avif",
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
            5 => Ok(TileType::Avif),
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
    /// If the byte buffer contains invalid `PMTiles` header data.
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
    #[expect(clippy::cast_possible_truncation)]
    fn write_coordinate_part<W: std::io::Write>(writer: &mut W, value: f64) -> std::io::Result<()> {
        writer.write_all(&((value * 10_000_000.0).round() as i32).to_le_bytes())
    }
}

#[cfg(test)]
mod tests {
    #![expect(clippy::unreadable_literal, clippy::float_cmp)]

    use std::fs::File;
    use std::io::Read;
    use std::num::NonZeroU64;

    use bytes::{Bytes, BytesMut};

    use crate::header::HEADER_SIZE;
    use crate::tests::{RASTER_FILE, VECTOR_FILE};
    use crate::{Header, TileType};

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
