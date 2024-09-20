#!/usr/bin/env just --justfile

@_default:
    just --list --unsorted

# Run cargo check
check:
    cargo check

_add_tools:
    rustup component add clippy rustfmt

# Run all tests
test:
    cargo test --features http-async
    cargo test --features mmap-async-tokio
    cargo test --features tilejson
    cargo test --features s3-async-native
    cargo test --features s3-async-rustls
    cargo test --features aws-s3-async
    cargo test
    RUSTDOCFLAGS="-D warnings" cargo doc --no-deps

# Run all tests and checks
test-all: check fmt clippy

# Run cargo fmt and cargo clippy
lint: fmt clippy

# Run cargo fmt
fmt: _add_tools
    cargo fmt --all -- --check

# Run cargo fmt using Rust nightly
fmt-nightly:
    cargo +nightly fmt -- --config imports_granularity=Module,group_imports=StdExternalCrate

# Run cargo clippy
clippy: _add_tools
    cargo clippy --workspace --all-targets --features http-async
    cargo clippy --workspace --all-targets --features mmap-async-tokio
    cargo clippy --workspace --all-targets --features tilejson
    cargo clippy --workspace --all-targets --features s3-async-native
    cargo clippy --workspace --all-targets --features s3-async-rustls
    cargo clippy --workspace --all-targets --features aws-s3-async

# Build and open code documentation
docs:
    cargo doc --no-deps --open

# Clean all build artifacts
clean:
    cargo clean
    rm -f Cargo.lock
