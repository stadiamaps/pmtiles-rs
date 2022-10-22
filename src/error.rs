#[derive(Debug)]
pub enum Error {
    InvalidMagicNumber,
    UnsupportedPmTilesVersion,
    InvalidHeader,
    InvalidCompression,
    InvalidTileType,
    Reading,
    UnableToOpenMmapFile,
    InvalidEntry,
    Http(String),
}

impl From<std::io::Error> for Error {
    fn from(_: std::io::Error) -> Self {
        Self::Reading
    }
}
