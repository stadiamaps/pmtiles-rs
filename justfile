#!/usr/bin/env just --justfile

@_default:
    just --list --unsorted

# Run all tests
test:
    # These are the same tests that are run on CI. Eventually CI should just call into justfile
    cargo check
    rustup component add clippy rustfmt
    cargo fmt --all -- --check
    cargo clippy --all-targets --all-features -- -D warnings
    cargo test --all-targets --all-features
    cargo test --features http-async
    cargo test --features mmap-async-tokio
    cargo test --features tilejson
    cargo test --features s3-async-native
    cargo test --features s3-async-rustls
    cargo test
    RUSTDOCFLAGS="-D warnings" cargo doc --no-deps

# Run cargo fmt and cargo clippy
lint: fmt clippy

# Run cargo fmt
fmt:
    cargo +nightly fmt -- --config imports_granularity=Module,group_imports=StdExternalCrate

# Run cargo clippy
clippy:
    cargo clippy --workspace --all-targets --all-features --bins --tests --lib --benches -- -D warnings

# Build and open code documentation
docs:
    cargo doc --no-deps --open

# Clean all build artifacts
clean:
    cargo clean
    rm -f Cargo.lock
