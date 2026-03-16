use std::io::Write;

use flate2::write::GzEncoder;

use crate::{Compression, PmtError, PmtResult};

/// Trait for compression implementations.
/// Implement this to provide custom compression behavior.
pub trait Compressor {
    /// Returns the compression type for the `PMTiles` header.
    fn compression(&self) -> Compression;

    /// Create an encoder wrapping `output`, invoke `input` to write
    /// uncompressed data into it, then finalize the encoder.
    ///
    /// # Errors
    ///
    /// Returns an error if writing to `output` fails or the compression fails.
    fn compress(
        &self,
        output: &mut dyn Write,
        input: &mut dyn FnMut(&mut dyn Write) -> std::io::Result<()>,
    ) -> PmtResult<()>;
}

/// Passthrough (no compression).
pub(crate) struct NoCompression;

impl Compressor for NoCompression {
    fn compression(&self) -> Compression {
        Compression::None
    }

    fn compress(
        &self,
        output: &mut dyn Write,
        input: &mut dyn FnMut(&mut dyn Write) -> std::io::Result<()>,
    ) -> PmtResult<()> {
        input(output)?;
        Ok(())
    }
}

/// Gzip compression. Wraps [`flate2::Compression`] for level configuration.
#[derive(Default)]
pub(crate) struct GzipCompressor(pub(crate) flate2::Compression);

impl Compressor for GzipCompressor {
    fn compression(&self) -> Compression {
        Compression::Gzip
    }

    fn compress(
        &self,
        output: &mut dyn Write,
        input: &mut dyn FnMut(&mut dyn Write) -> std::io::Result<()>,
    ) -> PmtResult<()> {
        let mut encoder = GzEncoder::new(output, self.0);
        input(&mut encoder)?;
        encoder.finish()?;
        Ok(())
    }
}

/// Brotli compression. Wraps [`brotli::enc::BrotliEncoderParams`].
#[cfg(feature = "brotli")]
#[derive(Default)]
pub(crate) struct BrotliCompressor(pub(crate) brotli::enc::BrotliEncoderParams);

#[cfg(feature = "brotli")]
impl Compressor for BrotliCompressor {
    fn compression(&self) -> Compression {
        Compression::Brotli
    }

    fn compress(
        &self,
        output: &mut dyn Write,
        input: &mut dyn FnMut(&mut dyn Write) -> std::io::Result<()>,
    ) -> PmtResult<()> {
        let mut encoder = brotli::CompressorWriter::with_params(output, 4096, &self.0);
        input(&mut encoder)?;
        Ok(())
    }
}

/// Zstd compression with configurable level.
#[cfg(feature = "zstd")]
pub(crate) struct ZstdCompressor(pub(crate) i32);

#[cfg(feature = "zstd")]
impl Compressor for ZstdCompressor {
    fn compression(&self) -> Compression {
        Compression::Zstd
    }

    fn compress(
        &self,
        output: &mut dyn Write,
        input: &mut dyn FnMut(&mut dyn Write) -> std::io::Result<()>,
    ) -> PmtResult<()> {
        let mut encoder = zstd::stream::Encoder::new(output, self.0)?;
        input(&mut encoder)?;
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

    fn compress(
        &self,
        _output: &mut dyn Write,
        _input: &mut dyn FnMut(&mut dyn Write) -> std::io::Result<()>,
    ) -> PmtResult<()> {
        Err(PmtError::UnsupportedCompression(self.0))
    }
}
