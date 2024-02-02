#[cfg(any(feature = "s3-async-native", feature = "s3-async-rustls"))]
mod s3;

#[cfg(any(feature = "s3-async-native", feature = "s3-async-rustls"))]
pub use s3::S3Backend;

#[cfg(feature = "http-async")]
mod http;

#[cfg(feature = "http-async")]
pub use http::HttpBackend;

#[cfg(feature = "mmap-async-tokio")]
mod mmap;

#[cfg(feature = "mmap-async-tokio")]
pub use mmap::MmapBackend;
