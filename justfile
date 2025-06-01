#!/usr/bin/env just --justfile

CRATE_NAME := "pmtiles"

@_default:
    just --list

# Quick compile without building a binary
check:
    RUSTFLAGS='-D warnings' cargo check --workspace --all-targets --features __all_non_conflicting

# Verify that the current version of the crate is not the same as the one published on crates.io
check-if-published:
    #!/usr/bin/env bash
    LOCAL_VERSION="$({{just_executable()}} get-crate-field version)"
    echo "Detected crate version:  $LOCAL_VERSION"
    CRATE_NAME="$({{just_executable()}} get-crate-field name)"
    echo "Detected crate name:     $CRATE_NAME"
    PUBLISHED_VERSION="$(cargo search ${CRATE_NAME} | grep "^${CRATE_NAME} =" | sed -E 's/.* = "(.*)".*/\1/')"
    echo "Published crate version: $PUBLISHED_VERSION"
    if [ "$LOCAL_VERSION" = "$PUBLISHED_VERSION" ]; then
        echo "ERROR: The current crate version has already been published."
        exit 1
    else
        echo "The current crate version has not yet been published."
    fi

# Generate code coverage report to upload to codecov.io
ci-coverage: && \
            (coverage '--codecov --output-path target/llvm-cov/codecov.info')
    # ATTENTION: the full file path above is used in the CI workflow
    mkdir -p target/llvm-cov

# Run all tests as expected by CI
ci-test: env-info test-fmt clippy check test test-doc

# Run minimal subset of tests to ensure compatibility with MSRV
ci-test-msrv: env-info check test

# Clean all build artifacts
clean:
    cargo clean
    rm -f Cargo.lock

# Run cargo clippy to lint the code
clippy:
    cargo clippy --workspace --all-targets --features __all_non_conflicting
    cargo clippy --workspace --all-targets --features s3-async-native

# Generate code coverage report
coverage *ARGS="--no-clean --open":
    cargo llvm-cov --workspace --all-targets --features __all_non_conflicting --include-build-script {{ARGS}}

# Build and open code documentation
docs:
    cargo doc --no-deps --open --features __all_non_conflicting

# Print environment info
env-info:
    @echo "Running on {{os()}} / {{arch()}}"
    {{just_executable()}} --version
    rustc --version
    cargo --version
    rustup --version

# Reformat all code `cargo fmt`. If nightly is available, use it for better results
fmt:
    #!/usr/bin/env bash
    set -euo pipefail
    if rustup component list --toolchain nightly | grep rustfmt &> /dev/null; then
        echo 'Reformatting Rust code using nightly Rust fmt to sort imports'
        cargo +nightly fmt --all -- --config imports_granularity=Module,group_imports=StdExternalCrate
    else
        echo 'Reformatting Rust with the stable cargo fmt.  Install nightly with `rustup install nightly` for better results'
        cargo fmt --all
    fi

# Get any package's field from the metadata
get-crate-field field package=CRATE_NAME:
    cargo metadata --format-version 1 | jq -r '.packages | map(select(.name == "{{package}}")) | first | .{{field}}'

# Get the minimum supported Rust version (MSRV) for the crate
get-msrv: (get-crate-field "rust_version")

# Run cargo fmt and cargo clippy
lint: fmt clippy

# Find the minimum supported Rust version (MSRV) using cargo-msrv extension, and update Cargo.toml
msrv:
    cargo msrv find --write-msrv --ignore-lockfile --features __all_non_conflicting

# Check semver compatibility with prior published version. Install it with `cargo install cargo-semver-checks`
semver *ARGS:
    cargo semver-checks {{ARGS}}

# Run all tests
test:
    #!/usr/bin/env bash
    set -euo pipefail
    export RUSTFLAGS='-D warnings'
    cargo test --features __all_non_conflicting
    cargo test --features s3-async-native
    cargo test

# Test documentation
test-doc:
    #!/usr/bin/env bash
    set -euo pipefail
    export RUSTDOCFLAGS="-D warnings"
    cargo test --doc --features __all_non_conflicting
    cargo test --doc --features s3-async-native
    cargo doc --no-deps --features __all_non_conflicting

# Test code formatting
test-fmt:
    cargo fmt --all -- --check

# Find unused dependencies. Install it with `cargo install cargo-udeps`
udeps:
    cargo +nightly udeps --all-targets --workspace --features __all_non_conflicting

# Update all dependencies, including breaking changes. Requires nightly toolchain (install with `rustup install nightly`)
update:
    cargo +nightly -Z unstable-options update --breaking
    cargo update
