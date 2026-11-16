use std::io::{self, Write};

#[cfg(feature = "brotli")]
use brotli::CompressorWriter;
#[cfg(feature = "brotli")]
use brotli::enc::BrotliEncoderParams;
use flate2::write::GzEncoder;

use crate::{Compression, PmtError, PmtResult};

/// Trait for compression implementations.
/// Implement this to provide custom compression behavior.
/// `Send`, so a writer can be moved to the thread that feeds it.
pub trait Compressor: Send {
    /// Returns the compression type for the `PMTiles` header.
    fn compression(&self) -> Compression;

    /// Invoke `f` to write uncompressed data into an encoder
    /// wrapping `writer`, then finalize the encoder.
    ///
    /// # Errors
    ///
    /// Returns an error if writing to `output` fails or the compression fails.
    fn compress(
        &self,
        f: &mut dyn FnMut(&mut dyn Write) -> io::Result<()>,
        writer: &mut dyn Write,
    ) -> PmtResult<()>;
}

impl<T: Compressor + ?Sized> Compressor for Box<T> {
    fn compression(&self) -> Compression {
        (**self).compression()
    }

    fn compress(
        &self,
        f: &mut dyn FnMut(&mut dyn Write) -> io::Result<()>,
        writer: &mut dyn Write,
    ) -> PmtResult<()> {
        (**self).compress(f, writer)
    }
}

/// Passthrough (no compression).
pub(crate) struct NoCompression;

impl Compressor for NoCompression {
    fn compression(&self) -> Compression {
        Compression::None
    }

    fn compress(
        &self,
        f: &mut dyn FnMut(&mut dyn Write) -> io::Result<()>,
        writer: &mut dyn Write,
    ) -> PmtResult<()> {
        f(writer)?;
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
        f: &mut dyn FnMut(&mut dyn Write) -> io::Result<()>,
        writer: &mut dyn Write,
    ) -> PmtResult<()> {
        let mut encoder = GzEncoder::new(writer, self.0);
        f(&mut encoder)?;
        encoder.finish()?;
        Ok(())
    }
}

/// Brotli compression. Wraps [`brotli::enc::BrotliEncoderParams`].
#[cfg(feature = "brotli")]
#[derive(Default)]
pub(crate) struct BrotliCompressor(pub(crate) BrotliEncoderParams);

#[cfg(feature = "brotli")]
impl Compressor for BrotliCompressor {
    fn compression(&self) -> Compression {
        Compression::Brotli
    }

    fn compress(
        &self,
        f: &mut dyn FnMut(&mut dyn Write) -> io::Result<()>,
        writer: &mut dyn Write,
    ) -> PmtResult<()> {
        let mut output = KeepError {
            inner: writer,
            error: None,
        };
        let mut encoder = CompressorWriter::with_params(&mut output, 4096, &self.0);
        let result = f(&mut encoder);
        // Writes the last block, ignoring any error
        encoder.into_inner();
        // The writer's own error, rather than the copy the encoder passed on
        match output.error {
            Some(err) => Err(err.into()),
            None => Ok(result?),
        }
    }
}

/// Keeps the first error of the writer it wraps, because the Brotli encoder ignores errors when
/// it writes its last block. It also retries interrupted writes, which the encoder does not.
#[cfg(feature = "brotli")]
struct KeepError<'a> {
    inner: &'a mut dyn Write,
    error: Option<io::Error>,
}

#[cfg(feature = "brotli")]
impl KeepError<'_> {
    /// Keeps the error of `result`, and returns a copy of it.
    fn keep<T>(&mut self, result: io::Result<T>) -> io::Result<T> {
        result.map_err(|err| {
            let kind = err.kind();
            self.error.get_or_insert(err);
            kind.into()
        })
    }
}

#[cfg(feature = "brotli")]
impl Write for KeepError<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let result = self.inner.write_all(buf);
        self.keep(result).map(|()| buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let result = self.inner.flush();
        self.keep(result)
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
        f: &mut dyn FnMut(&mut dyn Write) -> io::Result<()>,
        writer: &mut dyn Write,
    ) -> PmtResult<()> {
        let mut encoder = zstd::stream::Encoder::new(writer, self.0)?;
        f(&mut encoder)?;
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
        _f: &mut dyn FnMut(&mut dyn Write) -> io::Result<()>,
        _writer: &mut dyn Write,
    ) -> PmtResult<()> {
        Err(PmtError::UnsupportedCompression(self.0))
    }
}

#[cfg(test)]
mod tests {
    use std::io::{self, Write};

    use super::*;

    /// A writer that fails like one to a full disk: every flush, and every write if `writes_fail`.
    struct FullDisk {
        writes_fail: bool,
    }

    impl FullDisk {
        fn error() -> io::Error {
            io::Error::new(io::ErrorKind::StorageFull, "disk full")
        }
    }

    impl Write for FullDisk {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if self.writes_fail {
                Err(Self::error())
            } else {
                Ok(buf.len())
            }
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(Self::error())
        }
    }

    /// A failed write returns the writer's error with every compression, whether it happens while
    /// compressing a long input, or in the last block, which is where a short input is written.
    #[rstest::rstest]
    #[case(Compression::None)]
    #[case(Compression::Gzip)]
    #[cfg_attr(feature = "brotli", case(Compression::Brotli))]
    #[cfg_attr(feature = "zstd", case(Compression::Zstd))]
    fn write_errors_are_returned(
        #[case] compression: Compression,
        #[values(4, 1 << 16)] len: usize,
    ) {
        let data = incompressible(len);
        let compressor = Box::<dyn Compressor>::from(compression);
        let result = compressor.compress(
            &mut |w| w.write_all(&data),
            &mut FullDisk { writes_fail: true },
        );
        assert!(
            matches!(&result, Err(PmtError::Reading(err)) if err.to_string() == "disk full"),
            "{result:?}"
        );
    }

    /// A failed flush returns the writer's error with every compression.
    #[rstest::rstest]
    #[case(Compression::None)]
    #[case(Compression::Gzip)]
    #[cfg_attr(feature = "brotli", case(Compression::Brotli))]
    #[cfg_attr(feature = "zstd", case(Compression::Zstd))]
    fn flush_errors_are_returned(#[case] compression: Compression) {
        let compressor = Box::<dyn Compressor>::from(compression);
        let result = compressor.compress(&mut |w| w.flush(), &mut FullDisk { writes_fail: false });
        assert!(
            matches!(&result, Err(PmtError::Reading(err)) if err.to_string() == "disk full"),
            "{result:?}"
        );
    }

    /// An error from the input is returned with every compression when the writer has none.
    #[rstest::rstest]
    #[case(Compression::None)]
    #[case(Compression::Gzip)]
    #[cfg_attr(feature = "brotli", case(Compression::Brotli))]
    #[cfg_attr(feature = "zstd", case(Compression::Zstd))]
    fn input_errors_are_returned(#[case] compression: Compression) {
        let compressor = Box::<dyn Compressor>::from(compression);
        let result =
            compressor.compress(&mut |_| Err(io::Error::other("bad tile")), &mut Vec::new());
        assert!(
            matches!(&result, Err(PmtError::Reading(err)) if err.to_string() == "bad tile"),
            "{result:?}"
        );
    }

    /// Brotli returns the writer's own error from a write while compressing, not the copy it
    /// passed to the encoder. Its defaults buffer even a long input, so this uses a small window.
    #[cfg(feature = "brotli")]
    #[test]
    fn brotli_returns_errors_while_compressing() {
        let params = BrotliEncoderParams {
            quality: 1,
            lgwin: 10,
            ..BrotliEncoderParams::default()
        };
        let data = incompressible(1 << 16);
        let mut failed = false;
        let result = BrotliCompressor(params).compress(
            &mut |w| {
                let result = w.write_all(&data);
                failed = result.is_err();
                result
            },
            &mut FullDisk { writes_fail: true },
        );
        assert!(failed, "the write while compressing succeeded");
        assert!(
            matches!(&result, Err(PmtError::Reading(err)) if err.to_string() == "disk full"),
            "{result:?}"
        );
    }

    /// Pseudo-random bytes, which do not compress, so a long input is written before the end.
    fn incompressible(len: usize) -> Vec<u8> {
        let mut state = 1_u32;
        (0..len)
            .map(|_| {
                state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                state.to_le_bytes()[2]
            })
            .collect()
    }
}
