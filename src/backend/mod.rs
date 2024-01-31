#[cfg(any(feature = "s3-async-native", feature = "s3-async-rustls"))]
pub(crate) mod s3;

#[cfg(feature = "http-async")]
pub(crate) mod http;

#[cfg(feature = "mmap-async-tokio")]
pub(crate) mod mmap;
