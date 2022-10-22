# PMTiles (for Rust)

![Crates.io](https://img.shields.io/crates/v/pmtiles) ![GitHub branch checks state](https://img.shields.io/github/checks-status/stadiamaps/pmtiles-rs/main)

This crate implements the [PMTiles v3 spec](https://github.com/protomaps/PMTiles/blob/master/spec/v3/spec.md),
originally created by Brandon Liu for Protomaps.

**THIS CRATE IS NOT READY FOR PRODUCTION USE!** However, you might be able to use it for a demo project.

## Features

- Opening and validating PMTile archives
- Querying tiles
- Backends supported:
  - Async `mmap` (Tokio) for local files
  - Async `http` and `https` (Reqwuest + Tokio) for URLs

## Plans & TODOs

- [ ] Documentation and example code
- [ ] Support writing and conversion to and from MBTiles + `x/y/z`
- [ ] Support additional backends (sync `mmap` and `http` at least)
- [ ] Support additional async styles (e.g., `async-std`)

PRs welcome!

## License

This project is dual-licensed as MIT and Apache 2.0. You may select the license most appropriate for your project.

## Test Data License

Some PMTile fixtures copied from official [PMTiles repository](https://github.com/protomaps/PMTiles/commit/257b41dd0497e05d1d686aa92ce2f742b6251644).
