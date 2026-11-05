# `PMTiles` (for Rust)

[![GitHub repo](https://img.shields.io/badge/github-stadiamaps/pmtiles--rs-8da0cb?logo=github)](https://github.com/stadiamaps/pmtiles-rs)
[![crates.io version](https://img.shields.io/crates/v/pmtiles)](https://crates.io/crates/pmtiles)
[![docs.rs status](https://img.shields.io/docsrs/pmtiles)](https://docs.rs/pmtiles)
[![crates.io license](https://img.shields.io/crates/l/pmtiles)](https://github.com/stadiamaps/pmtiles-rs/blob/main/LICENSE-APACHE)
[![CI build status](https://github.com/stadiamaps/pmtiles-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/stadiamaps/pmtiles-rs/actions)
[![Codecov](https://img.shields.io/codecov/c/github/stadiamaps/pmtiles-rs)](https://app.codecov.io/gh/stadiamaps/pmtiles-rs)

This crate reads and writes [PMTiles v3](https://github.com/protomaps/PMTiles/blob/main/spec/v3/spec.md)
archives, the single-file tile format created by Brandon Liu for Protomaps.

## Features

- **Reading** tiles, metadata, [TileJSON](https://github.com/mapbox/tilejson-spec) and the list of all tile entries
- **Writing** archives, with identical tiles stored once
- **Backends**, all async on Tokio:
  - local files, read without blocking async threads on the disk, or memory-mapped
  - `http` and `https` URLs (Reqwest)
  - S3, with either the AWS SDK or Rust-S3
  - S3, Azure, GCP, local files, memory and custom stores through [`object_store`](https://docs.rs/object_store)
  - your own, by implementing `AsyncBackend`
- **Directory caches**: a simple `HashMapCache`, a bounded `MokaCache`, or your own `DirectoryCache`
- **Safe with untrusted archives**: invalid data returns an error instead of crashing, which CI checks by fuzzing
- **32-bit targets**, such as `i686`, read and write archives larger than 4 GiB (except through `MmapBackend`)
- **WebAssembly**: the core library builds for the browser, see [below](#webassembly)

## Cargo features

Only `file-async-tokio` and `iter` are enabled by default. Everything else is opt-in: most
backends pull in large dependency trees, and the AWS SDK alone is a heavy build.

```bash
cargo add pmtiles --features http-async,reqwest-rustls,moka,tilejson
```

### Reading

- `async`: `AsyncPmTilesReader`, the `AsyncBackend` trait to implement your own backend, and the
  `HashMapCache` and `NoCache` directory caches. It adds no dependencies and needs no particular
  async runtime, so it works with any executor, or in a browser.
- `tokio`: `async`, plus decoding large directories, metadata and tiles on Tokio's blocking
  thread pool, so that a slow decode does not hold up other tasks on the same thread. Decoding
  estimated to take longer than 100 µs moves there, and `blocking_decode_above` changes or
  disables that threshold. It only applies inside a Tokio runtime, which `pmtiles` never starts
  itself: outside one, everything decodes on the calling thread. Every backend enables it.
- `iter`: `entries()`, a stream of every tile entry in an archive, read through all leaf
  directories.
- `moka`: `MokaCache` and `SharedMokaCache`, directory caches that stay within a size limit.

`iter` and `moka` only extend the reader, so building with either of them but without `async` or
a backend fails.

### Backends

- `file-async-tokio`: `FileBackend`, which reads local files with positioned reads, and
  `AsyncPmTilesReader::new_with_path`. The preferred backend for local files, see
  [below](#reading-from-a-local-file).
- `mmap-async-tokio`: `MmapBackend`, which reads memory-mapped local files, and
  `AsyncPmTilesReader::new_with_mmap_path`. It suits only archives much smaller than the memory
  available, see [below](#memory-mapping-a-local-file).
- `http-async`: `HttpBackend`, which reads `http` URLs with Reqwest. For `https`, also enable one
  of `reqwest-rustls`, `reqwest-native-tls` (the platform's TLS library) or `reqwest-default`
  (Reqwest's default features, which use rustls).
- `aws-s3-async`: `AwsS3Backend`, which reads S3 with the AWS SDK.
- `s3-async-rustls` or `s3-async-native`: `S3Backend`, which reads S3 with Rust-S3, using rustls
  or the platform's TLS library. Enable only one of them.
- `object-store`: `ObjectStoreBackend`, which reads any [`object_store`](https://docs.rs/object_store)
  store: S3, Azure, GCP, local files, memory or your own.

### Writing and formats

- `write`: `PmTilesWriter`, which creates archives.
- `tilejson`: `parse_tilejson` on the reader and `get_tilejson` on `Header`, which build a
  [TileJSON](https://github.com/mapbox/tilejson-spec) from an archive's header and metadata.
- `brotli`, `zstd`: reading and writing archives compressed with Brotli or Zstandard. Gzip and
  uncompressed archives need no feature.

## WebAssembly

The `async`, `iter`, `write`, `tilejson`, `object-store`, `brotli` and `zstd` features build for
`wasm32-unknown-unknown`, and the tests run on `wasm32-wasip1`. CI checks both. The local file,
`http` and S3 backends and the `moka` cache do not build for wasm yet, so in a browser, read
archives through `object_store`, or your own `AsyncBackend` with only the `async` feature.

## Usage examples

### Reading from a local file

Data already in the page cache is read directly on Linux, and the rest on Tokio's blocking thread
pool. To update a file while a reader has it open, write a new file and rename it over the old
one: the reader keeps reading the old file. `FileBackendOptions` can check for in-place changes.

```rust,no_run
use bytes::Bytes;
use pmtiles::{AsyncPmTilesReader, PmtResult, TileCoord};

async fn get_tile(z: u8, x: u32, y: u32) -> PmtResult<Option<Bytes>> {
    let reader = AsyncPmTilesReader::new_with_path("example.pmtiles").await?;
    // `None` if the archive has no such tile
    reader.get_tile(TileCoord::new(z, x, y)?).await
}

async fn get_metadata() -> PmtResult<String> {
    let reader = AsyncPmTilesReader::new_with_path("example.pmtiles").await?;
    reader.get_metadata().await // JSON string
}
```

### Memory-mapping a local file

With the `mmap-async-tokio` feature, `MmapBackend` reads a memory-mapped file. Reads of data in
the page cache are not copied, which saves about 2.4 µs per 17 KB tile, but the `FileBackend`
above is the better choice in most cases. Use `MmapBackend` only if all of these hold:

- The archive is much smaller than the memory available to the process, including any container
  limit, so that it stays in the page cache. Otherwise every read of uncached data blocks an
  async worker thread on the disk, and reads up to 2 MiB around it: on a 139 GB archive, reads
  were up to 6.7 times slower.
- The file is never truncated or changed in place while it is mapped: that crashes the process.
- The target is 64-bit: on 32-bit targets, mapping an archive of more than 1-2 GiB can fail.

```rust,no_run
use pmtiles::{AsyncPmTilesReader, MmapBackend, PmtResult};

async fn open() -> PmtResult<AsyncPmTilesReader<MmapBackend>> {
    // Or `new_with_mmap_path_and_cache`, like `new_with_path_and_cache` below
    AsyncPmTilesReader::new_with_mmap_path("example.pmtiles").await
}
```

### Reading from a URL with a bounded cache

Every lookup reads one or more directories before the tile itself. A directory cache keeps them,
so repeated lookups skip those reads. `MokaCache` evicts directories to stay within its limit;
`HashMapCache` is simpler but never evicts.

```rust,no_run
use bytes::Bytes;
use pmtiles::reqwest::Client; // Re-exported Reqwest crate
use pmtiles::{AsyncPmTilesReader, MokaCache, PmtResult, TileCoord};

async fn get_tile(z: u8, x: u32, y: u32) -> PmtResult<Option<Bytes>> {
    // Use `MokaCache::new(100)` to limit the number of directories instead
    let cache = MokaCache::with_max_bytes(64 * 1024 * 1024);
    let client = Client::builder().use_rustls_tls().build()?;
    let url = "https://protomaps.github.io/PMTiles/protomaps(vector)ODbL_firenze.pmtiles";
    let reader = AsyncPmTilesReader::new_with_url_and_cache(client, url, cache).await?;
    reader.get_tile(TileCoord::new(z, x, y)?).await
}
```

### Sharing one cache budget between archives

A cache holds the directories of one archive. To give several archives one memory budget, create
each archive's cache from the same `SharedMokaCache`:

```rust,no_run
use pmtiles::{AsyncPmTilesReader, PmtResult, SharedMokaCache};

async fn open_archives() -> PmtResult<()> {
    let shared = SharedMokaCache::with_max_bytes(256 * 1024 * 1024);
    let a = AsyncPmTilesReader::new_with_path_and_cache("a.pmtiles", shared.new_cache()).await?;
    let b = AsyncPmTilesReader::new_with_path_and_cache("b.pmtiles", shared.new_cache()).await?;
    // ... read tiles from `a` and `b` ...
    println!("{} directories cached", shared.entry_count());
    Ok(())
}
```

For other caching needs, such as expiring entries, implement the `DirectoryCache` trait.

### Reading from an S3 bucket

See the [AWS SDK documentation](https://crates.io/crates/aws-sdk-s3) for configuring the client.

```rust,no_run
use bytes::Bytes;
use pmtiles::aws_sdk_s3::Client; // Re-exported AWS SDK S3 client
use pmtiles::{AsyncPmTilesReader, HashMapCache, PmtResult, TileCoord};

async fn get_tile(client: Client, z: u8, x: u32, y: u32) -> PmtResult<Option<Bytes>> {
    let cache = HashMapCache::default();
    let bucket = "my-bucket".to_string();
    let key = "example.pmtiles".to_string();
    let reader =
        AsyncPmTilesReader::new_with_client_bucket_path_and_cache(client, bucket, key, cache)
            .await?;
    reader.get_tile(TileCoord::new(z, x, y)?).await
}
```

### Writing a file

```rust,no_run
use std::fs::File;

use pmtiles::{PmTilesWriter, PmtResult, TileCoord, TileType};

fn write_archive() -> PmtResult<()> {
    let file = File::create("example.pmtiles")?;
    let mut writer = PmTilesWriter::new(TileType::Mvt)
        .metadata(r#"{"vector_layers": []}"#)
        .create(file)?;
    writer.add_tile(TileCoord::new(0, 0, 0)?, &[/* tile data */])?;
    writer.finalize()
}
```

## Development

* This project is easier to develop with [just](https://github.com/casey/just#readme), a modern alternative to `make`.
  Install it with `cargo install just`.
* To get a list of available commands, run `just`.
* To run tests, use `just test`.
* To fuzz the reader with malformed archives, use `just fuzz` (needs nightly Rust).
* To benchmark tile lookups, use `just bench`.
* To build and test for WebAssembly, use `just check-wasm` and `just test-wasm`.

## License

Licensed under either of

* Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <https://www.apache.org/licenses/LICENSE-2.0>)
* MIT license ([LICENSE-MIT](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)
  at your option.

### Test Data License

Some `PMTile` fixtures copied from official [PMTiles repository](https://github.com/protomaps/PMTiles/commit/257b41dd0497e05d1d686aa92ce2f742b6251644).

### Contribution

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the
Apache-2.0 license, shall be dual licensed as above, without any
additional terms or conditions.
