# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.19.1](https://github.com/stadiamaps/pmtiles-rs/compare/v0.19.0...v0.19.1) - 2026-01-13

### Other

- Add read and write support for brotli compression. ([#99](https://github.com/stadiamaps/pmtiles-rs/pull/99))

## [0.19.0](https://github.com/stadiamaps/pmtiles-rs/compare/v0.18.2...v0.19.0) - 2026-01-09

### Other

- *(deps)* [**breaking**] bump `reqwest` and `object_store` dependencys, modifying the list of available features ([#98](https://github.com/stadiamaps/pmtiles-rs/pull/98))
- Bump actions/checkout from 5 to 6 in the all-actions-version-updates group ([#94](https://github.com/stadiamaps/pmtiles-rs/pull/94))

## [0.18.2](https://github.com/stadiamaps/pmtiles-rs/compare/v0.18.1...v0.18.2) - 2025-11-13

### Other

- Add AVIF to the list of tile types ([#92](https://github.com/stadiamaps/pmtiles-rs/pull/92))

## [0.18.1](https://github.com/stadiamaps/pmtiles-rs/compare/v0.18.0...v0.18.1) - 2025-11-11

### Other

- Added a new atomic Caching interface ([#90](https://github.com/stadiamaps/pmtiles-rs/pull/90))

## [0.18.0](https://github.com/stadiamaps/pmtiles-rs/compare/v0.17.1...v0.18.0) - 2025-10-29

### Other

- Don't ignore errors from Gzip `finish` ([#89](https://github.com/stadiamaps/pmtiles-rs/pull/89))
- **Breaking:** losing lat/lon precision (10e-7) ([#87](https://github.com/stadiamaps/pmtiles-rs/pull/87))

## [0.17.1](https://github.com/stadiamaps/pmtiles-rs/compare/v0.17.0...v0.17.1) - 2025-10-13

### Fixed

- make the `object-store` feature depend on `__async` ([#84](https://github.com/stadiamaps/pmtiles-rs/pull/84))

### Other

- use crates.io trusted publishing ([#86](https://github.com/stadiamaps/pmtiles-rs/pull/86))
- move the backends to its own module ([#82](https://github.com/stadiamaps/pmtiles-rs/pull/82))
- document all doc items ([#83](https://github.com/stadiamaps/pmtiles-rs/pull/83))

## [0.17.0](https://github.com/stadiamaps/pmtiles-rs/compare/v0.16.0...v0.17.0) - 2025-09-23

### Other

- minor justfile/ci updates ([#81](https://github.com/stadiamaps/pmtiles-rs/pull/81))
- Upgrade rust-s3 ([#80](https://github.com/stadiamaps/pmtiles-rs/pull/80))
- cleanup Cargo.toml ([#79](https://github.com/stadiamaps/pmtiles-rs/pull/79))
- Writer tile dedup ([#76](https://github.com/stadiamaps/pmtiles-rs/pull/76))
- Fix range index errors on writing ([#75](https://github.com/stadiamaps/pmtiles-rs/pull/75))

## [0.16.0](https://github.com/stadiamaps/pmtiles-rs/compare/v0.15.0...v0.16.0) - 2025-09-08

### Added

- *(object_store)* add an `object_store::ObjectStore` backend ([#71](https://github.com/stadiamaps/pmtiles-rs/pull/71))
- `Coord::new` now returns Result ([#69](https://github.com/stadiamaps/pmtiles-rs/pull/69))

### Other

- Add an add_raw_tile that skips tile compression ([#74](https://github.com/stadiamaps/pmtiles-rs/pull/74))
- [pre-commit.ci] pre-commit autoupdate ([#73](https://github.com/stadiamaps/pmtiles-rs/pull/73))
- Bump actions/checkout from 4 to 5 in the all-actions-version-updates group ([#72](https://github.com/stadiamaps/pmtiles-rs/pull/72))

## [0.15.0](https://github.com/stadiamaps/pmtiles-rs/compare/v0.14.0...v0.15.0) - 2025-07-02

### Added

- add `AsyncPmTilesReader::entries` to iterate all entries ([#49](https://github.com/stadiamaps/pmtiles-rs/pull/49))
- rework tile coordinates ([#62](https://github.com/stadiamaps/pmtiles-rs/pull/62))

### Fixed

- fix readme link and obsolete docs badge ([#60](https://github.com/stadiamaps/pmtiles-rs/pull/60))

### Other

- enable all non-conflicting features by default ([#68](https://github.com/stadiamaps/pmtiles-rs/pull/68))
- automate release process ([#66](https://github.com/stadiamaps/pmtiles-rs/pull/66))
- use `u32` instead of `u64` for `(x,y)` ([#65](https://github.com/stadiamaps/pmtiles-rs/pull/65))
- prevent/autofix tabs in text ([#64](https://github.com/stadiamaps/pmtiles-rs/pull/64))
- upgrade to 2025 edition ([#61](https://github.com/stadiamaps/pmtiles-rs/pull/61))
