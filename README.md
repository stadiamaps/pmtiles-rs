# PMTiles (for Rust)

[![GitHub](https://img.shields.io/badge/github-stadiamaps/pmtiles--rs-8da0cb?logo=github)](https://github.com/stadiamaps/pmtiles-rs)
[![crates.io version](https://img.shields.io/crates/v/pmtiles.svg)](https://crates.io/crates/pmtiles)
[![docs.rs docs](https://docs.rs/pmtiles/badge.svg)](https://docs.rs/pmtiles)
[![crates.io version](https://img.shields.io/crates/l/pmtiles.svg)](https://github.com/stadiamaps/pmtiles-rs/blob/main/LICENSE-APACHE)
[![CI build](https://github.com/stadiamaps/pmtiles-rs/workflows/CI/badge.svg)](https://github.com/stadiamaps/pmtiles-rs/actions)

This crate implements the [PMTiles v3 spec](https://github.com/protomaps/PMTiles/blob/master/spec/v3/spec.md),
originally created by Brandon Liu for Protomaps.

## Features

- Opening and validating PMTile archives
- Querying tiles
- Backends supported:
  - Async `mmap` (Tokio) for local files
  - Async `http` and `https` (Reqwuest + Tokio) for URLs
  - Async `s3` (Rust-S3 + Tokio) for S3-compatible buckets
- Creating PMTile archives

## Plans & TODOs

- [ ] Documentation and example code
- [ ] Support conversion to and from MBTiles + `x/y/z`
- [ ] Support additional backends (sync `mmap` and `http` at least)
- [ ] Support additional async styles (e.g., `async-std`)

PRs welcome!

## Usage examples

### Writing a PMTiles file

```rust
use pmtiles::{PmTilesWriter, TileType};
use std::fs::File;

let file = File::create("tiles.pmtiles").unwrap();
let mut writer = PmTilesWriter::new(TileType::Mvt).create(file).unwrap();
writer.add_tile(0, &[/*...*/]).unwrap();
writer.finalize().unwrap();
```

## Development
* This project is easier to develop with [just](https://github.com/casey/just#readme), a modern alternative to `make`. Install it with `cargo install just`.
* To get a list of available commands, run `just`.
* To run tests, use `just test`.

## License

Licensed under either of

* Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <http://www.apache.org/licenses/LICENSE-2.0>)
* MIT license ([LICENSE-MIT](LICENSE-MIT) or <http://opensource.org/licenses/MIT>)
  at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the
Apache-2.0 license, shall be dual licensed as above, without any
additional terms or conditions.

## Test Data License

Some PMTile fixtures copied from official [PMTiles repository](https://github.com/protomaps/PMTiles/commit/257b41dd0497e05d1d686aa92ce2f742b6251644).
