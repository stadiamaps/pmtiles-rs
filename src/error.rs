use std::string::FromUtf8Error;

#[derive(Debug)]
pub enum Error {
    InvalidMagicNumber,
    UnsupportedPmTilesVersion,
    InvalidCompression,
    InvalidEntry,
    InvalidHeader,
    InvalidMetadata,
    InvalidTileType,
    Reading(std::io::Error),
    #[cfg(feature = "fmmap")]
    UnableToOpenMmapFile,
    Http(String),
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Reading(e)
    }
}

impl From<FromUtf8Error> for Error {
    fn from(_: FromUtf8Error) -> Self {
        Self::InvalidMetadata
    }
}
