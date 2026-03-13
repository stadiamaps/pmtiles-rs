use std::io::Write;

use flate2::write::GzEncoder;

use crate::{Compression, PmtError, PmtResult};

/// Trait for compression implementations.
/// Implement this to provide custom compression behavior.
pub trait Compressor {
    /// Returns the compression type for the `PMTiles` header.
    fn compression(&self) -> Compression;

    /// Compress `input` and write the compressed data to `output`.
    ///
    /// # Errors
    ///
    /// Returns an error if writing to `output` fails or the compression fails.
    fn compress(&self, input: &[u8], output: &mut dyn Write) -> PmtResult<()>;
}

/// Passthrough (no compression).
pub struct NoCompression;

impl Compressor for NoCompression {
    fn compression(&self) -> Compression {
        Compression::None
    }

    fn compress(&self, input: &[u8], output: &mut dyn Write) -> PmtResult<()> {
        output.write_all(input)?;
        Ok(())
    }
}

/// Gzip compression. Wraps [`flate2::Compression`] for level configuration.
#[derive(Default)]
pub struct GzipCompressor(pub flate2::Compression);

impl Compressor for GzipCompressor {
    fn compression(&self) -> Compression {
        Compression::Gzip
    }

    fn compress(&self, input: &[u8], output: &mut dyn Write) -> PmtResult<()> {
        let mut encoder = GzEncoder::new(output, self.0);
        encoder.write_all(input)?;
        encoder.finish()?;
        Ok(())
    }
}

/// Brotli compression. Wraps [`brotli::enc::BrotliEncoderParams`].
#[cfg(feature = "brotli")]
#[derive(Default)]
pub struct BrotliCompressor(pub brotli::enc::BrotliEncoderParams);

#[cfg(feature = "brotli")]
impl Compressor for BrotliCompressor {
    fn compression(&self) -> Compression {
        Compression::Brotli
    }

    fn compress(&self, input: &[u8], output: &mut dyn Write) -> PmtResult<()> {
        let mut encoder = brotli::CompressorWriter::with_params(output, 4096, &self.0);
        encoder.write_all(input)?;
        encoder.flush()?;
        Ok(())
    }
}

/// Zstd compression with configurable level.
#[cfg(feature = "zstd")]
pub struct ZstdCompressor(pub i32);

#[cfg(feature = "zstd")]
impl Compressor for ZstdCompressor {
    fn compression(&self) -> Compression {
        Compression::Zstd
    }

    fn compress(&self, input: &[u8], output: &mut dyn Write) -> PmtResult<()> {
        let mut encoder = zstd::stream::Encoder::new(output, self.0)?;
        encoder.write_all(input)?;
        encoder.finish()?;
        Ok(())
    }
}

#[cfg(feature = "zstd")]
impl Default for ZstdCompressor {
    fn default() -> Self {
        Self(zstd::DEFAULT_COMPRESSION_LEVEL)
    }
}

impl From<Compression> for Box<dyn Compressor> {
    fn from(compression: Compression) -> Self {
        match compression {
            Compression::None => Box::new(NoCompression),
            Compression::Gzip => Box::new(GzipCompressor::default()),
            #[cfg(feature = "brotli")]
            Compression::Brotli => Box::new(BrotliCompressor::default()),
            #[cfg(feature = "zstd")]
            Compression::Zstd => Box::new(ZstdCompressor::default()),
            v => Box::new(UnsupportedCompressor(v)),
        }
    }
}

/// Stub compressor for codecs whose feature is disabled.
/// Returns an error when compression is attempted.
struct UnsupportedCompressor(Compression);

impl Compressor for UnsupportedCompressor {
    fn compression(&self) -> Compression {
        self.0
    }

    fn compress(&self, _input: &[u8], _output: &mut dyn Write) -> PmtResult<()> {
        Err(PmtError::UnsupportedCompression(self.0))
    }
}
