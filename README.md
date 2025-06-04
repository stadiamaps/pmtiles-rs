# `PMTiles` (for Rust)

[![GitHub repo](https://img.shields.io/badge/github-stadiamaps/pmtiles--rs-8da0cb?logo=github)](https://github.com/stadiamaps/pmtiles-rs)
[![crates.io version](https://img.shields.io/crates/v/pmtiles)](https://crates.io/crates/pmtiles)
[![docs.rs status](https://img.shields.io/docsrs/pmtiles)](https://docs.rs/pmtiles)
[![crates.io license](https://img.shields.io/crates/l/pmtiles)](https://github.com/stadiamaps/pmtiles-rs/blob/main/LICENSE-APACHE)
[![CI build status](https://github.com/stadiamaps/pmtiles-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/stadiamaps/pmtiles-rs/actions)
[![Codecov](https://img.shields.io/codecov/c/github/stadiamaps/pmtiles-rs)](https://app.codecov.io/gh/stadiamaps/pmtiles-rs)

This crate implements the [PMTiles v3 spec](https://github.com/protomaps/PMTiles/blob/master/spec/v3/spec.md),
originally created by Brandon Liu for Protomaps.

## Features

- Opening and validating `PMTile` archives
- Querying tiles
- Backends supported:
  - Async `mmap` (Tokio) for local files
  - Async `http` and `https` (Reqwest + Tokio) for URLs
  - Async `s3` (Rust-S3 + Tokio) for S3-compatible buckets
- Creating `PMTile` archives

## Plans & TODOs

- [ ] Documentation and example code
- [ ] Support conversion to and from `MBTiles` + `x/y/z`
- [ ] Support additional backends (sync `mmap` and `http` at least)
- [ ] Support additional async styles (e.g., `async-std`)

PRs welcome!

## Usage examples

### Reading from a local `PMTiles` file

```rust,no_run
use bytes::Bytes;
use pmtiles::async_reader::AsyncPmTilesReader;

async fn get_tile(z: u8, x: u64, y: u64) -> Option<Bytes> {
  let file = "example.pmtiles";
  // Use `new_with_cached_path` for better performance
  let reader = AsyncPmTilesReader::new_with_path(file).await.unwrap();
  reader.get_tile(z, x, y).await.unwrap()
}
```

### Reading from a URL with a simple directory cache

This example uses a simple hashmap-based cache to optimize reads from a `PMTiles` source. The same caching is available for all other methods.  Note that `HashMapCache` is a rudimentary cache without eviction. You may want to build a more sophisticated cache for production use by implementing the `DirectoryCache` trait.

```rust,no_run
use bytes::Bytes;
use pmtiles::async_reader::AsyncPmTilesReader;
use pmtiles::cache::HashMapCache;
use pmtiles::reqwest::Client;  // Re-exported Reqwest crate

async fn get_tile(z: u8, x: u64, y: u64) -> Option<Bytes> {
  let cache = HashMapCache::default();
  let client = Client::builder().use_rustls_tls().build().unwrap();
  let url = "https://protomaps.github.io/PMTiles/protomaps(vector)ODbL_firenze.pmtiles";
  let reader = AsyncPmTilesReader::new_with_cached_url(cache, client, url).await.unwrap();
  reader.get_tile(z, x, y).await.unwrap()
}
```

### Reading from an S3 bucket with a directory cache

AWS client configuration is fairly none-trivial to document here. See AWS SDK [documentation](https://crates.io/crates/aws-sdk-s3) for more details.

```rust,no_run
use bytes::Bytes;
use pmtiles::async_reader::AsyncPmTilesReader;
use pmtiles::aws_sdk_s3::Client; // Re-exported AWS SDK S3 client
use pmtiles::cache::HashMapCache;

async fn get_tile(client: Client, z: u8, x: u64, y: u64) -> Option<Bytes> {
  let cache = HashMapCache::default();
  let bucket = "https://s3.example.com".to_string();
  let key = "example.pmtiles".to_string();
  let reader = AsyncPmTilesReader::new_with_cached_client_bucket_and_path(cache, client, bucket, key).await.unwrap();
  reader.get_tile(z, x, y).await.unwrap()
}
```

### Writing to a `PMTiles` file

```rust,no_run
use pmtiles::{PmTilesWriter, TileType};
use std::fs::File;

let file = File::create("example.pmtiles").unwrap();
let mut writer = PmTilesWriter::new(TileType::Mvt).create(file).unwrap();
writer.add_tile(0, 0, 0, &[/*...*/]).unwrap();
writer.finalize().unwrap();
```

## Development

* This project is easier to develop with [just](https://github.com/casey/just#readme), a modern alternative to `make`.
  Install it with `cargo install just`.
* To get a list of available commands, run `just`.
* To run tests, use `just test`.

## License

Licensed under either of

* Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <http://www.apache.org/licenses/LICENSE-2.0>)
* MIT license ([LICENSE-MIT](LICENSE-MIT) or <http://opensource.org/licenses/MIT>)
  at your option.

### Test Data License

Some `PMTile` fixtures copied from official [PMTiles repository](https://github.com/protomaps/PMTiles/commit/257b41dd0497e05d1d686aa92ce2f742b6251644).

### Contribution

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the
Apache-2.0 license, shall be dual licensed as above, without any
additional terms or conditions.
