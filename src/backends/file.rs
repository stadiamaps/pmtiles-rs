use std::fs::{File, Metadata};
use std::io;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt as _;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime};

use bytes::Bytes;
use tokio::task::spawn_blocking;

use crate::async_reader::read_full_at;
use crate::{
    AsyncBackend, AsyncPmTilesReader, BackendResponse, DirectoryCache, NoCache, PmtError, PmtResult,
};

impl AsyncPmTilesReader<FileBackend, NoCache> {
    /// Creates a new `PMTiles` reader from a file path using the [`FileBackend`], the preferred
    /// backend for local files.
    ///
    /// Fails if `path` does not exist or is an invalid archive.
    ///
    /// # Errors
    ///
    /// This function will return an error if the
    /// - file cannot be opened,
    /// - backend fails to read the header/root directory or
    /// - root directory is malformed
    pub async fn new_with_path<P: AsRef<Path>>(path: P) -> PmtResult<Self> {
        Self::new_with_path_and_cache(path, NoCache).await
    }
}

impl<C: DirectoryCache + Sync + Send> AsyncPmTilesReader<FileBackend, C> {
    /// Creates a new `PMTiles` reader with a directory cache from a file path using the
    /// [`FileBackend`], the preferred backend for local files.
    ///
    /// Fails if `path` does not exist or is an invalid archive.
    ///
    /// # Errors
    ///
    /// This function will return an error if the
    /// - file cannot be opened,
    /// - backend fails to read the header/root directory or
    /// - root directory is malformed
    pub async fn new_with_path_and_cache<P: AsRef<Path>>(path: P, cache: C) -> PmtResult<Self> {
        let backend = FileBackend::try_from(path).await?;

        Self::try_from_source_and_cache(backend, cache).await
    }

    /// Creates a new `PMTiles` reader with a directory cache from a file path using the
    /// [`FileBackend`].
    ///
    /// # Errors
    ///
    /// See [`Self::new_with_path_and_cache`].
    #[deprecated(
        since = "0.27.0",
        note = "renamed to `new_with_path_and_cache`, which takes the cache last"
    )]
    pub async fn new_with_cached_path<P: AsRef<Path>>(cache: C, path: P) -> PmtResult<Self> {
        Self::new_with_path_and_cache(path, cache).await
    }
}

/// Backend for reading `PMTiles` from a local file, and the preferred one.
///
/// [`AsyncPmTilesReader::new_with_path`] and [`AsyncPmTilesReader::new_with_path_and_cache`]
/// use it. It reads archives of any size, larger than memory or than the address space of a
/// 32-bit target, without blocking async worker threads on the disk. The memory-mapped
/// `MmapBackend`, with the `mmap-async-tokio` feature, reads cached data without copying it,
/// but only suits archives much smaller than the memory available to the process.
///
/// Each read copies its range of the file into a new buffer. On Linux, a read first takes what
/// the page cache already holds, without waiting for the disk (`RWF_NOWAIT`). Anything else is
/// read on Tokio's blocking thread pool, so waiting for the disk never blocks an async worker
/// thread. On other platforms, every read runs on the blocking pool. This requires a Tokio
/// runtime.
///
/// Use [`FileBackendOptions`] to open a file with non-default options.
///
/// # Changes to the file
///
/// To update an archive on Unix, write the new version to another file in the same directory and
/// rename it over the old one. The rename is atomic, and this backend keeps reading the old
/// file, which the operating system keeps until it is closed. Create a new backend to read the
/// new version.
///
/// Changing the file in place cannot crash the process, but reads may then return a mix of old
/// and new data, or fail. Enable [`FileBackendOptions::check_modified`] to detect either kind of
/// change, or the file being deleted.
pub struct FileBackend {
    file: Arc<File>,
    /// The file's length when it was opened. Reads are clamped to it.
    len: u64,
    /// Whether reads first try the page cache. Cleared if the file system does not support it.
    #[cfg(target_os = "linux")]
    try_cached: AtomicBool,
    /// Checks the file for changes, if enabled.
    modified_check: Option<ModifiedCheck>,
}

/// Options for opening a [`FileBackend`], all off by default.
///
/// ```no_run
/// # async fn test() -> pmtiles::PmtResult<()> {
/// use std::time::Duration;
///
/// use pmtiles::{AsyncPmTilesReader, FileBackendOptions};
///
/// let backend = FileBackendOptions::new()
///     .check_modified(Some(Duration::from_secs(1)))
///     .open("example.pmtiles")
///     .await?;
/// let reader = AsyncPmTilesReader::try_from_source(backend).await?;
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Copy, Debug, Default)]
pub struct FileBackendOptions {
    check_modified: Option<Duration>,
}

impl FileBackendOptions {
    /// Creates the default options.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Checks the file's path at most once per `interval`, and fails reads with
    /// [`PmtError::SourceModified`] once the file there was replaced or deleted, or its length or
    /// modification time changed since it was opened. `None` disables the check, and
    /// `Some(Duration::ZERO)` checks on every read.
    ///
    /// This catches most in-place changes within `interval`. On Windows, a file renamed over the
    /// path is caught only if its length or modification time differs. When several reads run at
    /// once, only one of them runs each check. A check runs on Tokio's blocking thread pool,
    /// because on a network file system it may wait for the server, so checking on every read
    /// makes reads of cached data noticeably slower.
    #[must_use]
    pub fn check_modified(mut self, interval: Option<Duration>) -> Self {
        self.check_modified = interval;
        self
    }

    /// Opens a file with these options.
    ///
    /// # Errors
    ///
    /// This function will return an error if the file cannot be opened.
    pub async fn open<P: AsRef<Path>>(self, path: P) -> PmtResult<FileBackend> {
        let path = path.as_ref().to_path_buf();
        let (path, file, metadata) = spawn_blocking(move || {
            let file = File::open(&path)?;
            let metadata = file.metadata()?;
            Ok::<_, io::Error>((path, file, metadata))
        })
        .await
        .map_err(io::Error::other)??;

        Ok(FileBackend {
            file: Arc::new(file),
            len: metadata.len(),
            #[cfg(target_os = "linux")]
            try_cached: AtomicBool::new(true),
            modified_check: self.check_modified.map(|interval| {
                ModifiedCheck::new(path.into(), FileVersion::from(&metadata), interval)
            }),
        })
    }
}

impl FileBackend {
    /// Opens a file with the default [`FileBackendOptions`].
    ///
    /// See [`FileBackend`] for how to update the file while it is open.
    ///
    /// # Errors
    ///
    /// This function will return an error if the file cannot be opened.
    pub async fn try_from<P: AsRef<Path>>(path: P) -> PmtResult<Self> {
        FileBackendOptions::new().open(path).await
    }

    /// Fails if checking for changes is enabled, and the file was changed, replaced or deleted
    /// since it was opened.
    async fn check_unmodified(&self) -> PmtResult<()> {
        match &self.modified_check {
            Some(check) => check.check().await,
            None => Ok(()),
        }
    }

    /// Reads into `buf` what the page cache holds, from `offset`, returning the number of bytes.
    ///
    /// This is one system call on the calling thread, with no hop to the blocking pool.
    /// `tokio::fs::File` cannot do this: it has no positioned reads, so each read needs a seek
    /// on the blocking pool, and it allows one operation at a time. Its `io_uring` support needs
    /// `--cfg tokio_unstable` in the final binary, still seeks on the blocking pool, and is
    /// often blocked in containers.
    #[cfg(target_os = "linux")]
    fn read_cached(&self, buf: &mut [u8], offset: u64) -> usize {
        use rustix::io::{Errno, ReadWriteFlags, preadv2};

        if !self.try_cached.load(Ordering::Relaxed) {
            return 0;
        }
        let bufs = &mut [io::IoSliceMut::new(buf)];
        match preadv2(&*self.file, bufs, offset, ReadWriteFlags::NOWAIT) {
            Ok(len) => len,
            // The data is not in the page cache, or a signal interrupted the read
            Err(Errno::AGAIN | Errno::INTR) => 0,
            // The kernel or file system does not support `RWF_NOWAIT`
            Err(_) => {
                self.try_cached.store(false, Ordering::Relaxed);
                0
            }
        }
    }

    #[cfg(not(target_os = "linux"))]
    #[expect(clippy::unused_self, reason = "matches the Linux version")]
    fn read_cached(&self, _buf: &mut [u8], _offset: u64) -> usize {
        0
    }
}

impl AsyncBackend for FileBackend {
    async fn read(&self, offset: u64, length: usize) -> PmtResult<BackendResponse> {
        self.check_unmodified().await?;

        // An offset at exactly the end of the file is a valid empty read; past it is an error
        let Some(available) = self.len.checked_sub(offset) else {
            return Err(PmtError::Reading(io::ErrorKind::UnexpectedEof.into()));
        };
        // Short reads are the normal path here: the reader always asks for `MAX_INITIAL_BYTES`
        // up front, which is more than the total size of a small archive.
        let length = usize::try_from(available).map_or(length, |available| available.min(length));

        let mut buf = vec![0; length];
        let cached = self.read_cached(&mut buf, offset);
        if cached < length {
            let file = Arc::clone(&self.file);
            buf = spawn_blocking(move || {
                let (_, rest) = buf.split_at_mut(cached);
                // The file is shorter than when it was opened
                if read_full_at(&file, rest, offset + cached as u64)? < rest.len() {
                    return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
                }
                Ok(buf)
            })
            .await
            .map_err(io::Error::other)??;
        }
        Ok(BackendResponse::new(Bytes::from(buf)))
    }
}

/// The length and modification time of a file, which change when it is written to, and on Unix
/// its device and inode numbers.
#[derive(Debug, PartialEq, Eq)]
struct FileVersion {
    #[cfg(unix)]
    id: (u64, u64),
    len: u64,
    /// `None` on platforms that do not record modification times.
    modified: Option<SystemTime>,
}

impl From<&Metadata> for FileVersion {
    fn from(metadata: &Metadata) -> Self {
        Self {
            #[cfg(unix)]
            id: (metadata.dev(), metadata.ino()),
            len: metadata.len(),
            modified: metadata.modified().ok(),
        }
    }
}

/// Checks the file at a path for changes, at most once per interval.
struct ModifiedCheck {
    /// The path the file was opened from.
    path: Arc<Path>,
    /// The file's version when it was opened.
    version: FileVersion,
    interval: Duration,
    /// When the file was opened, which [`Self::next_check`] counts from.
    opened: Instant,
    /// Nanoseconds after `opened` when the next read checks the file.
    next_check: AtomicU64,
    /// Whether a change was found. Every read fails from then on, even within the interval.
    modified: AtomicBool,
}

impl ModifiedCheck {
    fn new(path: Arc<Path>, version: FileVersion, interval: Duration) -> Self {
        Self {
            path,
            version,
            interval,
            opened: Instant::now(),
            next_check: AtomicU64::new(nanos(interval)),
            modified: AtomicBool::new(false),
        }
    }

    /// Fails if the file at the path changed since it was opened, checking it if the interval
    /// has passed.
    async fn check(&self) -> PmtResult<()> {
        if self.modified.load(Ordering::Relaxed) {
            return Err(PmtError::SourceModified);
        }
        // Reading the clock is much cheaper than the system call it may skip
        if !self.claim(nanos(self.opened.elapsed())) {
            return Ok(());
        }
        // A `stat` of the path, because an `fstat` of the open file would miss a rename over it
        let path = Arc::clone(&self.path);
        let changed = match spawn_blocking(move || path.metadata())
            .await
            .map_err(io::Error::other)?
        {
            Ok(metadata) => FileVersion::from(&metadata) != self.version,
            // The file was deleted, or renamed away
            Err(err) if err.kind() == io::ErrorKind::NotFound => true,
            Err(err) => return Err(err.into()),
        };
        if changed {
            self.modified.store(true, Ordering::Relaxed);
            return Err(PmtError::SourceModified);
        }
        Ok(())
    }

    /// Returns whether the read at `now` nanoseconds after opening should check the file, and if
    /// so, moves the next check one interval later. Of several reads that find the interval
    /// passed, only one claims the check; the others skip it.
    fn claim(&self, now: u64) -> bool {
        if self.interval.is_zero() {
            return true;
        }
        let next_check = self.next_check.load(Ordering::Relaxed);
        now >= next_check
            && self
                .next_check
                .compare_exchange(
                    next_check,
                    now.saturating_add(nanos(self.interval)),
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                )
                .is_ok()
    }
}

/// Converts a duration to nanoseconds, saturating after about 584 years.
fn nanos(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::fs::OpenOptions;
    use std::io::Write as _;
    #[cfg(target_os = "linux")]
    use std::num::NonZeroU64;
    use std::path::PathBuf;
    use std::{env, fs};

    #[cfg(target_os = "linux")]
    use rustix::fs::{Advice, fadvise};
    use tempfile::TempDir;

    use super::*;
    use crate::tests::RASTER_FILE;
    use crate::{HashMapCache, TileCoord};

    /// A tile that the raster fixture has.
    fn tile() -> TileCoord {
        TileCoord::new(3, 4, 2).unwrap()
    }

    /// Writes a file of `len` distinct-looking bytes to a temporary directory, which is deleted
    /// when it is dropped. The directory is in Cargo's target directory, not `/tmp`, which is
    /// often `tmpfs`, whose pages cannot be evicted.
    fn temp_file(len: usize) -> (TempDir, PathBuf, Vec<u8>) {
        let target = env::var_os("CARGO_TARGET_DIR").map_or_else(|| "target".into(), PathBuf::from);
        let dir = TempDir::new_in(target).unwrap();
        let path = dir.path().join("data.bin");
        let data: Vec<u8> = (0..len)
            .map(|i| u8::try_from(i * 31 % 251).unwrap())
            .collect();
        let mut file = File::create(&path).unwrap();
        file.write_all(&data).unwrap();
        // Written pages stay dirty until synced, and the page cache cannot evict dirty pages
        file.sync_all().unwrap();
        (dir, path, data)
    }

    /// Copies the raster fixture to a temporary directory.
    fn copy_fixture() -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("raster.pmtiles");
        fs::copy(RASTER_FILE, &path).unwrap();
        (dir, path)
    }

    /// Opens a cached reader, and checks that it reads a tile.
    async fn open_reader(
        path: &Path,
        options: FileBackendOptions,
    ) -> AsyncPmTilesReader<FileBackend, HashMapCache> {
        let backend = options.open(path).await.unwrap();
        let reader =
            AsyncPmTilesReader::try_from_source_and_cache(backend, HashMapCache::default())
                .await
                .unwrap();
        assert!(reader.get_tile(tile()).await.unwrap().is_some());
        reader
    }

    /// Appends to the file, changing its length.
    fn append(path: &Path) {
        OpenOptions::new()
            .append(true)
            .open(path)
            .unwrap()
            .write_all(b"more")
            .unwrap();
    }

    #[tokio::test]
    async fn reader_reads_tiles() {
        let reader = AsyncPmTilesReader::new_with_path(RASTER_FILE)
            .await
            .unwrap();
        assert!(reader.get_tile(tile()).await.unwrap().is_some());

        let reader =
            AsyncPmTilesReader::new_with_path_and_cache(RASTER_FILE, HashMapCache::default())
                .await
                .unwrap();
        assert!(reader.get_tile(tile()).await.unwrap().is_some());
    }

    /// The constructors from before the rename still work, with the cache first.
    #[tokio::test]
    #[expect(deprecated, reason = "tests the deprecated names")]
    async fn deprecated_constructors_read_tiles() {
        let reader = AsyncPmTilesReader::new_with_cached_path(HashMapCache::default(), RASTER_FILE)
            .await
            .unwrap();
        assert!(reader.get_tile(tile()).await.unwrap().is_some());

        let backend = FileBackend::try_from(RASTER_FILE).await.unwrap();
        let reader = AsyncPmTilesReader::try_from_cached_source(backend, HashMapCache::default())
            .await
            .unwrap();
        assert!(reader.get_tile(tile()).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn opening_a_missing_file_fails() {
        assert!(matches!(
            FileBackend::try_from("fixtures/missing.pmtiles").await,
            Err(PmtError::Reading(_))
        ));
    }

    #[tokio::test]
    async fn read_whole_file() {
        let (_dir, path, data) = temp_file(10_000);
        let backend = FileBackend::try_from(&path).await.unwrap();

        let res = backend.read(0, data.len()).await.unwrap();
        assert_eq!(res.bytes, data);
        assert!(res.data_version_string.is_none());
    }

    /// Clamping must account for `offset`, not just the total length.
    #[tokio::test]
    async fn read_clamps_to_eof() {
        let (_dir, path, data) = temp_file(10_000);
        let backend = FileBackend::try_from(&path).await.unwrap();

        let res = backend.read(0, 20_000).await.unwrap();
        assert_eq!(res.bytes, data);
        let res = backend.read(9_990, 100).await.unwrap();
        assert_eq!(res.bytes, data[9_990..]);
    }

    #[tokio::test]
    async fn read_at_eof_is_empty() {
        let (_dir, path, _) = temp_file(10_000);
        let backend = FileBackend::try_from(&path).await.unwrap();

        assert!(backend.read(10_000, 10).await.unwrap().bytes.is_empty());
    }

    #[tokio::test]
    async fn read_past_eof_errors() {
        let (_dir, path, _) = temp_file(10_000);
        let backend = FileBackend::try_from(&path).await.unwrap();

        assert!(matches!(
            backend.read(10_001, 10).await,
            Err(PmtError::Reading(_))
        ));
        assert!(matches!(
            backend.read(u64::MAX, 1).await,
            Err(PmtError::Reading(_))
        ));
    }

    #[tokio::test]
    async fn read_exact_past_eof_errors() {
        let (_dir, path, data) = temp_file(10_000);
        let backend = FileBackend::try_from(&path).await.unwrap();

        let res = backend.read_exact(20, 50).await.unwrap();
        assert_eq!(res.bytes, data[20..70]);
        assert!(matches!(
            backend.read_exact(9_995, 10).await,
            Err(PmtError::UnexpectedNumberOfBytesReturned(10, 5))
        ));
    }

    /// Drops a range of the file from the page cache.
    #[cfg(target_os = "linux")]
    fn evict(path: &Path, offset: u64, len: u64) {
        let file = File::open(path).unwrap();
        fadvise(&file, offset, NonZeroU64::new(len), Advice::DontNeed).unwrap();
    }

    /// With nothing cached, the whole read runs on the blocking pool.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn reads_uncached_bytes() {
        let (_dir, path, data) = temp_file(256 * 1024);
        evict(&path, 0, 0);
        let backend = FileBackend::try_from(&path).await.unwrap();
        assert_eq!(backend.read_cached(&mut [0; 10], 1000), 0);

        let res = backend.read(1000, 200_000).await.unwrap();
        assert_eq!(res.bytes, data[1000..201_000]);
    }

    /// A read that starts in cached pages takes those without blocking, and reads the rest on
    /// the blocking pool. The two parts must join without skipping or repeating bytes.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn reads_partly_cached_bytes() {
        let (_dir, path, data) = temp_file(256 * 1024);
        evict(&path, 0, 0);
        // Cache the first half, by reading it, then make sure the second half is not cached
        let _ = fs::read(&path).unwrap();
        evict(&path, 128 * 1024, 128 * 1024);
        let backend = FileBackend::try_from(&path).await.unwrap();
        let cached = backend.read_cached(&mut vec![0; 100_000], 100_000);
        assert_eq!(
            cached,
            128 * 1024 - 100_000,
            "only the first half should be cached"
        );

        let res = backend.read(100_000, 100_000).await.unwrap();
        assert_eq!(res.bytes, data[100_000..200_000]);
    }

    /// A file system that rejects `RWF_NOWAIT` turns page-cache reads off for good, and every
    /// read then runs on the blocking pool. Reading a directory fails, so it stands in for one.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn unsupported_cached_reads_are_turned_off() {
        let (dir, path, data) = temp_file(10_000);
        let backend = FileBackend::try_from(dir.path()).await.unwrap();
        assert_eq!(backend.read_cached(&mut [0; 10], 0), 0);
        assert!(!backend.try_cached.load(Ordering::Relaxed));

        // The file was just written, so its bytes are cached, but are not read from the cache
        let mut backend = FileBackend::try_from(&path).await.unwrap();
        *backend.try_cached.get_mut() = false;
        assert_eq!(backend.read_cached(&mut [0; 10], 0), 0);

        let res = backend.read(0, data.len()).await.unwrap();
        assert_eq!(res.bytes, data);
    }

    /// Truncating the file in place makes reads past the new end fail, without crashing.
    #[tokio::test]
    async fn truncating_fails_reads_without_crashing() {
        let (_dir, path, _) = temp_file(10_000);
        let backend = FileBackend::try_from(&path).await.unwrap();

        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(100)
            .unwrap();
        assert!(matches!(
            backend.read(0, 1000).await,
            Err(PmtError::Reading(_))
        ));
    }

    /// Without `check_modified`, an append is not noticed: reads stay within the length the
    /// file had when it was opened, so every tile stays readable.
    #[tokio::test]
    async fn appending_is_ignored_by_default() {
        let (_dir, path) = copy_fixture();
        let reader = open_reader(&path, FileBackendOptions::new()).await;

        append(&path);
        assert!(reader.get_tile(tile()).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn appending_is_source_modified() {
        let (_dir, path) = copy_fixture();
        let options = FileBackendOptions::new().check_modified(Some(Duration::ZERO));
        let reader = open_reader(&path, options).await;

        append(&path);
        assert!(matches!(
            reader.get_tile(tile()).await,
            Err(PmtError::SourceModified)
        ));
    }

    /// Only the first read after the interval passed checks the file, until the next interval.
    #[test]
    fn claims_each_check_once() {
        let version = FileVersion {
            #[cfg(unix)]
            id: (0, 0),
            len: 0,
            modified: None,
        };
        let check = ModifiedCheck::new(Path::new("").into(), version, Duration::from_nanos(100));
        assert!(!check.claim(99), "the first interval has not passed");
        assert!(check.claim(150));
        assert!(
            !check.claim(150),
            "a concurrent read must not repeat the check"
        );
        assert!(!check.claim(249));
        assert!(check.claim(250));
    }

    /// With a zero interval, every read checks the file, even concurrent ones.
    #[test]
    fn zero_interval_claims_every_check() {
        let version = FileVersion {
            #[cfg(unix)]
            id: (0, 0),
            len: 0,
            modified: None,
        };
        let check = ModifiedCheck::new(Path::new("").into(), version, Duration::ZERO);
        assert!(check.claim(0));
        assert!(check.claim(0));
        assert!(check.claim(1));
    }

    /// Within the interval, reads skip the check, so a change is not noticed yet.
    #[tokio::test]
    async fn appending_is_ignored_within_interval() {
        let (_dir, path) = copy_fixture();
        let options = FileBackendOptions::new().check_modified(Some(Duration::from_secs(3600)));
        let reader = open_reader(&path, options).await;

        append(&path);
        assert!(reader.get_tile(tile()).await.unwrap().is_some());
    }

    /// A write that keeps the length is still detected by the modification time, and every
    /// read fails from then on.
    #[tokio::test]
    async fn touching_is_source_modified() {
        let (_dir, path, _) = temp_file(10_000);
        let backend = FileBackendOptions::new()
            .check_modified(Some(Duration::ZERO))
            .open(&path)
            .await
            .unwrap();
        backend.read_exact(0, 10).await.unwrap();

        let file = File::options().write(true).open(&path).unwrap();
        let modified = file.metadata().unwrap().modified().unwrap();
        file.set_modified(modified + Duration::from_secs(1))
            .unwrap();
        assert!(matches!(
            backend.read_exact(0, 10).await,
            Err(PmtError::SourceModified)
        ));
        assert!(matches!(
            backend.read(0, 10).await,
            Err(PmtError::SourceModified)
        ));
    }

    /// Renaming another file over the open one is the safe way to update an archive. Without
    /// `check_modified`, the reader keeps reading the old file, until a new reader opens the new
    /// one.
    #[cfg(unix)]
    #[tokio::test]
    async fn replacing_by_rename_keeps_old_file() {
        let (dir, path) = copy_fixture();
        let reader = open_reader(&path, FileBackendOptions::new()).await;
        let before = reader.get_tile(tile()).await.unwrap();

        let replacement = dir.path().join("replacement.pmtiles");
        fs::write(&replacement, b"not an archive").unwrap();
        fs::rename(&replacement, &path).unwrap();

        assert_eq!(reader.get_tile(tile()).await.unwrap(), before);
        assert!(matches!(
            AsyncPmTilesReader::new_with_path(&path).await,
            Err(PmtError::InvalidMagicNumber)
        ));
    }

    /// A file renamed over the open one is caught by its inode, even with the same length and
    /// modification time.
    #[cfg(unix)]
    #[tokio::test]
    async fn replacing_by_rename_is_source_modified() {
        let (dir, path) = copy_fixture();
        let options = FileBackendOptions::new().check_modified(Some(Duration::ZERO));
        let reader = open_reader(&path, options).await;

        let replacement = dir.path().join("replacement.pmtiles");
        fs::copy(&path, &replacement).unwrap();
        let file = File::options().write(true).open(&replacement).unwrap();
        file.set_modified(fs::metadata(&path).unwrap().modified().unwrap())
            .unwrap();
        fs::rename(&replacement, &path).unwrap();
        assert!(matches!(
            reader.get_tile(tile()).await,
            Err(PmtError::SourceModified)
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn deleting_is_source_modified() {
        let (_dir, path) = copy_fixture();
        let options = FileBackendOptions::new().check_modified(Some(Duration::ZERO));
        let reader = open_reader(&path, options).await;

        fs::remove_file(&path).unwrap();
        assert!(matches!(
            reader.get_tile(tile()).await,
            Err(PmtError::SourceModified)
        ));
    }
}
