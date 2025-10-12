#[cfg(feature = "aws-s3-async")]
mod aws_s3;
#[cfg(feature = "aws-s3-async")]
pub use crate::backends::aws_s3::AwsS3Backend;
#[cfg(feature = "http-async")]
mod http;
#[cfg(feature = "http-async")]
pub use crate::backends::http::HttpBackend;
#[cfg(feature = "mmap-async-tokio")]
mod mmap;
#[cfg(feature = "mmap-async-tokio")]
pub use crate::backends::mmap::MmapBackend;
#[cfg(feature = "object-store")]
mod object_store;
#[cfg(feature = "object-store")]
pub use crate::backends::object_store::ObjectStoreBackend;
#[cfg(feature = "__async-s3")]
mod s3;
#[cfg(feature = "__async-s3")]
pub use crate::backends::s3::S3Backend;
