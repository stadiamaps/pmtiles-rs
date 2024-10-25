use std::string::FromUtf8Error;

use thiserror::Error;

use crate::Compression;

/// A specialized [`Result`] type for `PMTiles` operations.
pub type PmtResult<T> = Result<T, PmtError>;

/// Errors that can occur while reading `PMTiles` files.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum PmtError {
    #[error("Invalid magic number")]
    InvalidMagicNumber,
    #[error("Invalid PMTiles version")]
    UnsupportedPmTilesVersion,
    #[error("Invalid compression")]
    InvalidCompression,
    #[error("Unsupported compression {0:?}")]
    UnsupportedCompression(Compression),
    #[error("Invalid PMTiles entry")]
    InvalidEntry,
    #[error("Invalid header")]
    InvalidHeader,
    #[error("Invalid metadata")]
    InvalidMetadata,
    #[error("Directory index element overflow")]
    IndexEntryOverflow,
    #[error("Invalid metadata UTF-8 encoding: {0}")]
    InvalidMetadataUtf8Encoding(#[from] FromUtf8Error),
    #[error("Invalid tile type")]
    InvalidTileType,
    #[error("IO Error {0}")]
    Reading(#[from] std::io::Error),
    #[cfg(feature = "mmap-async-tokio")]
    #[error("Unable to open mmap file")]
    UnableToOpenMmapFile,
    #[error("Unexpected number of bytes returned [expected: {0}, received: {1}].")]
    UnexpectedNumberOfBytesReturned(usize, usize),
    #[cfg(feature = "http-async")]
    #[error("Range requests unsupported")]
    RangeRequestsUnsupported,
    #[cfg(any(
        feature = "http-async",
        feature = "__async-s3",
        feature = "__async-aws-s3"
    ))]
    #[error("HTTP response body is too long, Response {0}B > requested {1}B")]
    ResponseBodyTooLong(usize, usize),
    #[cfg(feature = "http-async")]
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[cfg(feature = "http-async")]
    #[error(transparent)]
    InvalidHeaderValue(#[from] reqwest::header::InvalidHeaderValue),
    #[cfg(feature = "__async-s3")]
    #[error(transparent)]
    S3(#[from] s3::error::S3Error),
    #[cfg(feature = "__async-aws-s3")]
    #[error(transparent)]
    AwsS3Request(
        #[from] aws_sdk_s3::error::SdkError<aws_sdk_s3::operation::get_object::GetObjectError>,
    ),
}
