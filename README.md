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
  - local files with `mmap`
  - `http` and `https` URLs (Reqwest)
  - S3, with either the AWS SDK or Rust-S3
  - S3, Azure, GCP, local files, memory and custom stores through [`object_store`](https://docs.rs/object_store)
  - your own, by implementing `AsyncBackend`
- **Directory caches**: a simple `HashMapCache`, a bounded `MokaCache`, or your own `DirectoryCache`
- **Safe with untrusted archives**: invalid data returns an error instead of crashing, which CI checks by fuzzing
- **32-bit targets**, such as `i686`, read and write archives larger than 4 GiB
- **WebAssembly**: the core library builds for the browser, see [below](#webassembly)

## Cargo features

Only `mmap-async-tokio` and `iter-async` are enabled by default. The remaining backends
pull in large dependency trees (the AWS SDK alone is a heavy build), so they are opt-in:

| Feature            | Enables                                                         |
|--------------------|-----------------------------------------------------------------|
| `write`            | Creating archives (`PmTilesWriter`)                             |
| `http-async`       | `http` and `https` backend (Reqwest)                            |
| `aws-s3-async`     | S3 backend using the AWS SDK                                    |
| `s3-async-rustls`  | S3 backend using Rust-S3 with rustls (or `s3-async-native`)     |
| `object-store`     | [`object_store`](https://docs.rs/object_store) backend          |
| `moka`             | Bounded directory caches based on `moka`                        |
| `tilejson`         | `TileJSON` support for metadata                                 |
| `brotli`, `zstd`   | Additional tile and directory compression codecs                |

```bash
cargo add pmtiles --features http-async,moka,tilejson
```

## WebAssembly

The `iter-async`, `write`, `tilejson`, `object-store`, `brotli` and `zstd` features build for
`wasm32-unknown-unknown`, and the tests run on `wasm32-wasip1`. CI checks both. The `mmap`,
`http` and S3 backends and the `moka` cache do not build for wasm yet, so in a browser, read
archives through `object_store` or your own `AsyncBackend`.

## Usage examples

### Reading from a local file

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
    let reader = AsyncPmTilesReader::new_with_cached_url(cache, client, url).await?;
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
    let a = AsyncPmTilesReader::new_with_cached_path(shared.new_cache(), "a.pmtiles").await?;
    let b = AsyncPmTilesReader::new_with_cached_path(shared.new_cache(), "b.pmtiles").await?;
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
        AsyncPmTilesReader::new_with_cached_client_bucket_and_path(cache, client, bucket, key)
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
