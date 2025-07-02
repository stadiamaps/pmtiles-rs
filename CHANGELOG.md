# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.14.1](https://github.com/stadiamaps/pmtiles-rs/compare/v0.14.0...v0.14.1) - 2025-07-02

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
