//! Benchmarks random tile reads from a local file, comparing [`FileBackend`] with
//! [`MmapBackend`].
//!
//! The archive has every tile of zoom levels 0 through 7, each between 1 and 32 KiB, so it is
//! far larger than the kernel's readahead window and tiles span several pages, as in real
//! archives. The runtime has 4 worker threads, like a small server. Cases:
//!
//! - `hot`: every page is already in the page cache, so a read costs no I/O.
//! - `cold` (Linux only): the file is evicted from the page cache before each batch of tiles,
//!   which are read one at a time from a new reader, so directories are read from disk too.
//! - `cold_dirs_cached`: as `cold`, but the directories stay cached, like in a long-running
//!   server, so that only tile data is read from disk.
//! - `cold_concurrent`: as `cold_dirs_cached`, but all tiles of a batch are read at once.
//!
//! Every byte of each tile is read, as sending it would: reads from a mapping are lazy, so
//! without that, the mapped tile data would never be read from disk. The archive is written to
//! Cargo's target temporary directory, not `/tmp`, which is often `tmpfs` and so cannot be
//! evicted. Results depend heavily on the disk.
//!
//! Run with `just bench local_file`, or `cargo bench --features file-async-tokio,mmap-async-tokio,write --bench local_file`.
#![expect(
    clippy::expect_used,
    reason = "a benchmark should stop on any setup failure"
)]

use std::fs::{File, rename};
use std::hint::black_box;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
#[cfg(target_os = "linux")]
use std::time::Instant;

use criterion::{Criterion, criterion_group, criterion_main};
use pmtiles::{
    AsyncBackend, AsyncPmTilesReader, BackendResponse, Compression, DirEntry, Directory,
    DirectoryCache, FileBackend, FileBackendOptions, HashMapCache, MmapBackend,
    PYRAMID_SIZE_BY_ZOOM, PmTilesWriter, PmtResult, TileId, TileType,
};
#[cfg(target_os = "linux")]
use rustix::fs::{Advice as FileAdvice, fadvise};
use tokio::runtime::{Builder as TokioBuilder, Runtime};
#[cfg(target_os = "linux")]
use tokio::task::JoinSet;

/// Number of tiles in zoom levels 0 through 7.
const TILE_COUNT: u64 = PYRAMID_SIZE_BY_ZOOM[8];

/// Number of distinct tile IDs that a benchmark cycles through.
const LOOKUP_COUNT: usize = 4096;

/// Number of tiles read after each eviction of the file in the cold benchmarks.
#[cfg(target_os = "linux")]
const COLD_BATCH: usize = 64;

/// A xorshift64 generator, so that tile sizes and lookups are random but repeatable.
struct XorShift(u64);

impl XorShift {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

/// Writes the archive, or reuses the one from a previous run.
fn archive_path() -> PathBuf {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join("local-file-bench.pmtiles");
    if path.exists() {
        return path;
    }

    let partial = path.with_extension("partial");
    let file = BufWriter::new(File::create(&partial).expect("file should be created"));
    let mut writer = PmTilesWriter::new(TileType::Mvt)
        .tile_compression(Compression::None)
        .create(file)
        .expect("writer should be created");
    let mut sizes = XorShift(0x9e37_79b9_7f4a_7c15);
    let mut tile = Vec::new();
    for id in 0..TILE_COUNT {
        let size = 1024 + usize::try_from(sizes.next() % (31 * 1024)).expect("fits in usize");
        // The leading ID keeps every tile distinct, so none are deduplicated
        tile.clear();
        tile.extend(id.to_le_bytes().iter().cycle().take(size));
        let coord = TileId::new(id).expect("tile ID should be valid").into();
        writer
            .add_tile(coord, &tile)
            .expect("tile should be written");
    }
    writer.finalize().expect("archive should be finalized");
    rename(&partial, &path).expect("archive should be renamed");
    path
}

/// Returns pseudo-random tile IDs, so consecutive lookups hit different parts of the file.
fn lookup_ids() -> Vec<TileId> {
    let mut state = XorShift(0x2545_f491_4f6c_dd1d);
    (0..LOOKUP_COUNT)
        .map(|_| TileId::new(state.next() % TILE_COUNT).expect("tile ID should be valid"))
        .collect()
}

/// How to open the archive.
#[derive(Clone, Copy)]
enum Open {
    File(FileBackendOptions),
    Mmap,
}

/// Either backend, so that both can be benchmarked the same way.
enum Backend {
    File(FileBackend),
    Mmap(MmapBackend),
}

impl AsyncBackend for Backend {
    async fn read_exact(&self, offset: u64, length: usize) -> PmtResult<BackendResponse> {
        match self {
            Self::File(backend) => backend.read_exact(offset, length).await,
            Self::Mmap(backend) => backend.read_exact(offset, length).await,
        }
    }

    async fn read(&self, offset: u64, length: usize) -> PmtResult<BackendResponse> {
        match self {
            Self::File(backend) => backend.read(offset, length).await,
            Self::Mmap(backend) => backend.read(offset, length).await,
        }
    }
}

/// A directory cache that outlives its reader, so that a new reader can start with it.
#[derive(Clone, Default)]
struct SharedCache(Arc<HashMapCache>);

impl DirectoryCache for SharedCache {
    async fn get_dir_entry_or_insert(
        &self,
        offset: u64,
        tile_id: TileId,
        fetcher: impl Future<Output = PmtResult<Directory>> + Send,
    ) -> PmtResult<Option<DirEntry>> {
        self.0
            .get_dir_entry_or_insert(offset, tile_id, fetcher)
            .await
    }
}

type Reader = AsyncPmTilesReader<Backend, SharedCache>;

async fn open(path: &Path, how: Open, cache: SharedCache) -> Reader {
    let backend = match how {
        Open::File(options) => Backend::File(options.open(path).await.expect("file should open")),
        Open::Mmap => Backend::Mmap(MmapBackend::try_from(path).await.expect("file should map")),
    };
    AsyncPmTilesReader::try_from_source_and_cache(backend, cache)
        .await
        .expect("archive should be readable")
}

/// Reads a tile and every byte of it, returning their sum.
async fn read_tile(reader: &Reader, id: TileId) -> u8 {
    let tile = reader.get_tile(id).await.expect("tile lookup should work");
    let tile = tile.expect("tile should exist");
    tile.iter().fold(0, |sum, byte| sum.wrapping_add(*byte))
}

/// Opens the archive and reads every tile once, so that all directories are in `cache`.
fn fill_cache(runtime: &Runtime, path: &Path, how: Open, cache: &SharedCache) {
    runtime.block_on(async {
        let reader = open(path, how, cache.clone()).await;
        for id in lookup_ids() {
            read_tile(&reader, id).await;
        }
    });
}

fn bench_hot(c: &mut Criterion, runtime: &Runtime, path: &Path, name: &str, how: Open) {
    let cache = SharedCache::default();
    // Reading every tile once caches both its pages and its directories
    fill_cache(runtime, path, how, &cache);
    let reader = runtime.block_on(open(path, how, cache));

    let ids = lookup_ids();
    let mut next = ids.iter().cycle();
    c.bench_function(&format!("local_file/hot/{name}"), |b| {
        b.to_async(runtime).iter(|| {
            let id = *next.next().expect("cycle never ends");
            let reader = &reader;
            async move { black_box(read_tile(reader, id).await) }
        });
    });
}

/// Drops the file's pages from the page cache. Pages that are still mapped are kept, so every
/// reader of the file must be dropped first.
#[cfg(target_os = "linux")]
fn evict(path: &Path) {
    let file = File::open(path).expect("file should open");
    fadvise(&file, 0, None, FileAdvice::DontNeed).expect("pages should be evicted");
}

/// How the cold benchmarks read each batch of tiles.
#[cfg(target_os = "linux")]
#[derive(Clone, Copy, PartialEq)]
enum Cold {
    /// One at a time, reading directories from disk too.
    Sequential,
    /// One at a time, with all directories cached.
    DirsCached,
    /// All at once, with all directories cached.
    Concurrent,
}

#[cfg(target_os = "linux")]
fn bench_cold(
    c: &mut Criterion,
    runtime: &Runtime,
    path: &Path,
    name: &str,
    how: Open,
    cold: Cold,
) {
    let group = match cold {
        Cold::Sequential => "local_file/cold",
        Cold::DirsCached => "local_file/cold_dirs_cached",
        Cold::Concurrent => "local_file/cold_concurrent",
    };
    let cache = SharedCache::default();
    if cold != Cold::Sequential {
        fill_cache(runtime, path, how, &cache);
    }

    let mut group = c.benchmark_group(group);
    group.sample_size(20);
    group.bench_function(name, |b| {
        let ids = lookup_ids();
        let mut batches = ids.chunks(COLD_BATCH).cycle();
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let batch = batches.next().expect("cycle never ends");
                evict(path);
                let cache = if cold == Cold::Sequential {
                    SharedCache::default()
                } else {
                    cache.clone()
                };
                // Only the tile reads are timed, not opening the archive
                total += runtime.block_on(async {
                    let reader = Arc::new(open(path, how, cache).await);
                    let start = Instant::now();
                    if cold == Cold::Concurrent {
                        let mut tasks = JoinSet::new();
                        for &id in batch {
                            let reader = Arc::clone(&reader);
                            tasks.spawn(async move { read_tile(&reader, id).await });
                        }
                        black_box(tasks.join_all().await);
                    } else {
                        for &id in batch {
                            black_box(read_tile(&reader, id).await);
                        }
                    }
                    start.elapsed()
                });
            }
            // Report the time per tile, to compare with the hot benchmark
            total / u32::try_from(COLD_BATCH).expect("fits in u32")
        });
    });
    group.finish();
}

fn local_file(c: &mut Criterion) {
    let runtime = TokioBuilder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("runtime should be created");
    let path = archive_path();

    let default = FileBackendOptions::new();
    let backends = [("file", Open::File(default)), ("mmap", Open::Mmap)];

    for (name, how) in backends {
        bench_hot(c, &runtime, &path, name, how);
    }
    let every_read = Open::File(default.check_modified(Some(Duration::ZERO)));
    bench_hot(
        c,
        &runtime,
        &path,
        "file_check_modified_every_read",
        every_read,
    );
    let every_second = Open::File(default.check_modified(Some(Duration::from_secs(1))));
    bench_hot(
        c,
        &runtime,
        &path,
        "file_check_modified_every_second",
        every_second,
    );

    #[cfg(target_os = "linux")]
    for cold in [Cold::Sequential, Cold::DirsCached, Cold::Concurrent] {
        for (name, how) in backends {
            bench_cold(c, &runtime, &path, name, how, cold);
        }
    }
}

criterion_group!(benches, local_file);
criterion_main!(benches);
