//! Benchmarks tile lookups through [`AsyncPmTilesReader`] with each directory cache.
//!
//! The archive has every tile of zoom levels 0 through 8, each with distinct content, so the
//! writer must put the entries into leaf directories holding thousands of entries each.
//! The archive is kept in memory, so the benchmark measures directory lookups, not I/O.
//!
//! Run with `just bench`, or `cargo bench --features moka,write --bench directory_cache`.

use std::hint::black_box;
use std::io::Cursor;

use bytes::Bytes;
use criterion::{Criterion, criterion_group, criterion_main};
use pmtiles::{
    AsyncBackend, AsyncPmTilesReader, BackendResponse, Compression, DirectoryCache, HashMapCache,
    MokaCache, NoCache, PYRAMID_SIZE_BY_ZOOM, PmTilesWriter, PmtResult, TileId, TileType,
};
use tokio::runtime::Runtime;

/// Number of tiles in zoom levels 0 through 8.
const TILE_COUNT: u64 = PYRAMID_SIZE_BY_ZOOM[9];

/// Number of distinct tile IDs that a benchmark cycles through.
const LOOKUP_COUNT: usize = 4096;

struct MemoryBackend(Bytes);

impl AsyncBackend for MemoryBackend {
    fn read(
        &self,
        offset: usize,
        length: usize,
    ) -> impl Future<Output = PmtResult<BackendResponse>> + Send {
        let start = offset.min(self.0.len());
        let end = offset.saturating_add(length).min(self.0.len());
        let bytes = self.0.slice(start..end);
        async move { Ok(BackendResponse::new(bytes)) }
    }
}

fn build_archive() -> Bytes {
    let mut buffer = Cursor::new(Vec::new());
    let mut writer = PmTilesWriter::new(TileType::Mvt)
        .tile_compression(Compression::None)
        .dedup_by_key::<u64>()
        .create(&mut buffer)
        .expect("writer should be created");
    for id in 0..TILE_COUNT {
        let coord = TileId::new(id).expect("tile ID should be valid").into();
        writer
            .add_tile(coord, &id.to_le_bytes(), None)
            .expect("tile should be written");
    }
    writer.finalize().expect("archive should be finalized");
    Bytes::from(buffer.into_inner())
}

/// Returns pseudo-random tile IDs, so consecutive lookups hit different leaf directories.
fn lookup_ids() -> Vec<TileId> {
    let mut state: u64 = 0x2545_f491_4f6c_dd1d;
    (0..LOOKUP_COUNT)
        .map(|_| {
            // xorshift64
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            TileId::new(state % TILE_COUNT).expect("tile ID should be valid")
        })
        .collect()
}

fn bench_get_tile<C: DirectoryCache + Sync + Send>(
    c: &mut Criterion,
    runtime: &Runtime,
    archive: &Bytes,
    name: &str,
    cache: C,
) {
    let ids = lookup_ids();
    let reader = runtime
        .block_on(AsyncPmTilesReader::try_from_cached_source(
            MemoryBackend(archive.clone()),
            cache,
        ))
        .expect("archive should be readable");

    let tile = runtime
        .block_on(reader.get_tile(ids[0]))
        .expect("tile lookup should work");
    assert!(tile.is_some(), "tile {:?} should exist", ids[0]);

    let mut next = ids.iter().cycle();
    c.bench_function(&format!("get_tile/{name}"), |b| {
        b.to_async(runtime).iter(|| {
            let id = *next.next().expect("cycle never ends");
            let reader = &reader;
            async move { black_box(reader.get_tile(id).await.expect("tile lookup should work")) }
        });
    });
}

fn directory_cache(c: &mut Criterion) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("runtime should be created");
    let archive = build_archive();

    bench_get_tile(c, &runtime, &archive, "no_cache", NoCache);
    bench_get_tile(
        c,
        &runtime,
        &archive,
        "hash_map_cache",
        HashMapCache::default(),
    );
    bench_get_tile(
        c,
        &runtime,
        &archive,
        "moka_cache",
        MokaCache {
            cache: moka::future::Cache::new(100),
        },
    );
}

criterion_group!(benches, directory_cache);
criterion_main!(benches);
