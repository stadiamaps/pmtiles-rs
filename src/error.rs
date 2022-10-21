#[derive(Debug)]
pub enum Error {
    InvalidMagicNumber,
    InvalidPmTilesVersion,
    InvalidHeader,
    InvalidCompression,
    InvalidTileType,
    ReadError,
    UnableToOpenMmapFile,
    InvalidEntry,
    HttpError(String),
}

impl From<std::io::Error> for Error {
    fn from(_: std::io::Error) -> Self {
        Self::ReadError
    }
}
