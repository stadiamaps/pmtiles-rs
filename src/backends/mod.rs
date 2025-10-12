#[cfg(feature = "aws-s3-async")]
mod aws_s3;
#[cfg(feature = "http-async")]
mod http;
#[cfg(feature = "mmap-async-tokio")]
mod mmap;
#[cfg(feature = "object-store")]
mod object_store;
#[cfg(feature = "__async-s3")]
mod s3;
