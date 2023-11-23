use std::string::FromUtf8Error;

use thiserror::Error;

/// A specialized [`Result`] type for `PMTiles` operations.
pub type PmtResult<T> = Result<T, PmtError>;

/// Errors that can occur while reading `PMTiles` files.
#[derive(Debug, Error)]
pub enum PmtError {
    #[error("Invalid magic number")]
    InvalidMagicNumber,
    #[error("Invalid PMTiles version")]
    UnsupportedPmTilesVersion,
    #[error("Invalid compression")]
    InvalidCompression,
    #[error("Invalid PMTiles entry")]
    InvalidEntry,
    #[error("Invalid header")]
    InvalidHeader,
    #[error("Invalid metadata")]
    InvalidMetadata,
    #[error("Invalid metadata UTF-8 encoding: {0}")]
    InvalidMetadataUtf8Encoding(#[from] FromUtf8Error),
    #[error("Invalid tile type")]
    InvalidTileType,
    #[error("IO Error {0}")]
    Reading(#[from] std::io::Error),
    #[cfg(feature = "mmap-async-tokio")]
    #[error("Unable to open mmap file")]
    UnableToOpenMmapFile,
    #[cfg(feature = "http-async")]
    #[error("{0}")]
    Http(#[from] PmtHttpError),
}

#[cfg(feature = "http-async")]
#[derive(Debug, Error)]
pub enum PmtHttpError {
    #[error("Unexpected number of bytes returned [expected: {0}, received: {1}].")]
    UnexpectedNumberOfBytesReturned(usize, usize),
    #[error("Range requests unsupported")]
    RangeRequestsUnsupported,
    #[error("HTTP response body is too long, Response {0}B > requested {1}B")]
    ResponseBodyTooLong(usize, usize),
    #[error("HTTP error {0}")]
    Http(#[from] reqwest::Error),
    #[error("{0}")]
    InvalidHeaderValue(#[from] reqwest::header::InvalidHeaderValue),
}

// This is required because thiserror #[from] does not support two-level conversion.
#[cfg(feature = "http-async")]
impl From<reqwest::Error> for PmtError {
    fn from(e: reqwest::Error) -> Self {
        Self::Http(PmtHttpError::Http(e))
    }
}
