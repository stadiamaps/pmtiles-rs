#[derive(Debug)]
pub enum Error {
    InvalidMagicNumber,
    UnsupportedPmTilesVersion,
    InvalidHeader,
    InvalidCompression,
    InvalidTileType,
    Reading(std::io::Error),
    #[cfg(any(feature = "fmmap", test))]
    UnableToOpenMmapFile,
    InvalidEntry,
    Http(String),
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Reading(e)
    }
}
