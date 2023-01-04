use std::string::FromUtf8Error;

#[derive(Debug)]
pub enum Error {
    InvalidMagicNumber,
    UnsupportedPmTilesVersion,
    InvalidCompression,
    InvalidEntry,
    InvalidHeader,
    InvalidMetadata,
    InvalidMetadataJson(serde_json::Error),
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

impl From<serde_json::Error> for Error {
    fn from(err: serde_json::Error) -> Self {
        Self::InvalidMetadataJson(err)
    }
}
