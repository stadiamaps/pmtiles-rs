#!/usr/bin/env just --justfile

@_default:
    just --list

_add_tools:
    rustup component add clippy rustfmt

# Clean all build artifacts
clean:
    cargo clean
    rm -f Cargo.lock

# Update all dependencies, including breaking changes. Requires nightly toolchain (install with `rustup install nightly`)
update:
    cargo +nightly -Z unstable-options update --breaking
    cargo update

# Find unused dependencies. Install it with `cargo install cargo-udeps`
udeps:
    cargo +nightly udeps --all-targets --workspace --all-features

# Check semver compatibility with prior published version. Install it with `cargo install cargo-semver-checks`
semver *ARGS:
    cargo semver-checks {{ARGS}}

# Find the minimum supported Rust version (MSRV) using cargo-msrv extension, and update Cargo.toml
msrv:
    cargo msrv find --write-msrv --ignore-lockfile

# Run cargo clippy to lint the code
clippy: _add_tools
    cargo clippy --workspace --all-targets --features http-async
    cargo clippy --workspace --all-targets --features mmap-async-tokio
    cargo clippy --workspace --all-targets --features tilejson
    cargo clippy --workspace --all-targets --features s3-async-native
    cargo clippy --workspace --all-targets --features s3-async-rustls
    cargo clippy --workspace --all-targets --features aws-s3-async

# Run all tests and checks
test-all: check fmt clippy

# Run cargo fmt and cargo clippy
lint: fmt clippy

# Test code formatting
test-fmt:
    cargo fmt --all -- --check

# Reformat all code `cargo fmt`. If nightly is available, use it for better results
fmt: _add_tools
    #!/usr/bin/env bash
    set -euo pipefail
    if command -v cargo +nightly &> /dev/null; then
        echo 'Reformatting Rust code using nightly Rust fmt to sort imports'
        cargo +nightly fmt --all -- --config imports_granularity=Module,group_imports=StdExternalCrate
    else
        echo 'Reformatting Rust with the stable cargo fmt.  Install nightly with `rustup install nightly` for better results'
        cargo fmt --all
    fi

# Build and open code documentation
docs:
    cargo doc --no-deps --open

# Quick compile without building a binary
check:
    RUSTFLAGS='-D warnings' cargo check --workspace --all-targets

# Generate code coverage report
coverage *ARGS="--no-clean --open":
    cargo llvm-cov --workspace --all-targets --include-build-script {{ARGS}}

# Generate code coverage report to upload to codecov.io
ci-coverage: && \
            (coverage '--codecov --output-path target/llvm-cov/codecov.info')
    # ATTENTION: the full file path above is used in the CI workflow
    mkdir -p target/llvm-cov

# Run all tests
test:
    RUSTFLAGS='-D warnings' cargo test --features http-async
    RUSTFLAGS='-D warnings' cargo test --features mmap-async-tokio
    RUSTFLAGS='-D warnings' cargo test --features tilejson
    RUSTFLAGS='-D warnings' cargo test --features s3-async-native
    RUSTFLAGS='-D warnings' cargo test --features s3-async-rustls
    RUSTFLAGS='-D warnings' cargo test --features aws-s3-async
    RUSTFLAGS='-D warnings' cargo test

# Test documentation
test-doc:
    RUSTDOCFLAGS="-D warnings" cargo test --doc
    RUSTDOCFLAGS="-D warnings" cargo doc --no-deps

# Print Rust version information
@rust-info:
    rustc --version
    cargo --version

# Run all tests as expected by CI
ci-test: rust-info test-fmt clippy check test test-doc

# Run minimal subset of tests to ensure compatibility with MSRV
ci-test-msrv: rust-info check test

# Verify that the current version of the crate is not the same as the one published on crates.io
check-if-published:
    #!/usr/bin/env bash
    LOCAL_VERSION="$(grep '^version =' Cargo.toml | sed -E 's/version = "([^"]*)".*/\1/')"
    echo "Detected crate version:  $LOCAL_VERSION"
    CRATE_NAME="$(grep '^name =' Cargo.toml | head -1 | sed -E 's/name = "(.*)"/\1/')"
    echo "Detected crate name:     $CRATE_NAME"
    PUBLISHED_VERSION="$(cargo search ${CRATE_NAME} | grep "^${CRATE_NAME} =" | sed -E 's/.* = "(.*)".*/\1/')"
    echo "Published crate version: $PUBLISHED_VERSION"
    if [ "$LOCAL_VERSION" = "$PUBLISHED_VERSION" ]; then
        echo "ERROR: The current crate version has already been published."
        exit 1
    else
        echo "The current crate version has not yet been published."
    fi
