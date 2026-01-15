#!/usr/bin/env just --justfile

main_crate := 'pmtiles'
# How to call the current just executable. Note that just_executable() may have `\` in Windows paths, so we need to quote it.
just := quote(just_executable())
# cargo-binstall needs a workaround due to caching when used in CI
binstall_args := if env('CI', '') != '' {'--no-confirm --no-track --disable-telemetry'} else {''}

# if running in CI, treat warnings as errors by setting RUSTFLAGS and RUSTDOCFLAGS to '-D warnings' unless they are already set
# Use `CI=true just ci-test` to run the same tests as in GitHub CI.
# Use `just env-info` to see the current values of RUSTFLAGS and RUSTDOCFLAGS
ci_mode := if env('CI', '') != '' {'1'} else {''}
export RUSTFLAGS := env('RUSTFLAGS', if ci_mode == '1' {'-D warnings'} else {''})
export RUSTDOCFLAGS := env('RUSTDOCFLAGS', if ci_mode == '1' {'-D warnings'} else {''})
export RUST_BACKTRACE := env('RUST_BACKTRACE', if ci_mode == '1' {'1'} else {'0'})

@_default:
    {{just}} --list

# Build the project
build:
    cargo build --workspace --features default --all-targets

# Quick compile without building a binary
check:
    cargo check --workspace --features default --all-targets
    @echo "--------------  Checking individual crate features"
    cargo check --workspace --all-targets --no-default-features --features aws-s3-async
    cargo check --workspace --all-targets --no-default-features --features http-async
    cargo check --workspace --all-targets --no-default-features --features iter-async
    cargo check --workspace --all-targets --no-default-features --features mmap-async-tokio
    cargo check --workspace --all-targets --no-default-features --features object-store
    cargo check --workspace --all-targets --no-default-features --features s3-async-native
    cargo check --workspace --all-targets --no-default-features --features s3-async-rustls
    cargo check --workspace --all-targets --no-default-features --features tilejson
    cargo check --workspace --all-targets --no-default-features --features write
    cargo check --workspace --all-targets --no-default-features --features brotli

# Generate code coverage report to upload to codecov.io
ci-coverage: env-info && \
            (coverage '--codecov --output-path target/llvm-cov/codecov.info')
    # ATTENTION: the full file path above is used in the CI workflow
    mkdir -p target/llvm-cov

# Run all tests as expected by CI
ci-test: env-info test-fmt clippy check test test-doc deny && assert-git-is-clean

# Run minimal subset of tests to ensure compatibility with MSRV
ci-test-msrv:
    if [ ! -f Cargo.lock.bak ]; then  mv Cargo.lock Cargo.lock.bak ; fi
    cp Cargo.lock.msrv Cargo.lock
    {{just}} env-info test
    rm Cargo.lock
    mv Cargo.lock.bak Cargo.lock

# Clean all build artifacts
clean:
    cargo clean
    rm -f Cargo.lock

# Run cargo clippy to lint the code
clippy *args:
    cargo clippy --workspace --features default --all-targets {{args}}
    cargo clippy --workspace --all-targets --no-default-features --features s3-async-native {{args}}

# Generate code coverage report. Will install `cargo llvm-cov` if missing.
coverage *args='--no-clean --open':  (cargo-install 'cargo-llvm-cov')
    cargo llvm-cov --workspace --features default --all-targets --include-build-script {{args}}

deny *args='check': (cargo-install 'cargo-deny')
    cargo deny {{args}}

# Build and open code documentation
docs *args='--open':
    DOCS_RS=1 cargo doc --no-deps {{args}} --workspace --features default

# Print environment info
env-info:
    @echo "Running for '{{main_crate}}' crate {{if ci_mode == '1' {'in CI mode'} else {'in dev mode'} }} on {{os()}} / {{arch()}}"
    @echo "PWD {{justfile_directory()}}"
    {{just}} --version
    rustc --version
    cargo --version
    rustup --version
    @echo "RUSTFLAGS='$RUSTFLAGS'"
    @echo "RUSTDOCFLAGS='$RUSTDOCFLAGS'"
    @echo "RUST_BACKTRACE='$RUST_BACKTRACE'"

# Reformat all code `cargo fmt`. If nightly is available, use it for better results
fmt:
    #!/usr/bin/env bash
    set -euo pipefail
    if (rustup toolchain list | grep nightly && rustup component list --toolchain nightly | grep rustfmt) &> /dev/null; then
        echo 'Reformatting Rust code using nightly Rust fmt to sort imports'
        cargo +nightly fmt --all -- --config imports_granularity=Module,group_imports=StdExternalCrate
    else
        echo 'Reformatting Rust with the stable cargo fmt.  Install nightly with `rustup install nightly` for better results'
        cargo fmt --all
    fi

# Reformat all Cargo.toml files using cargo-sort
fmt-toml *args:  (cargo-install 'cargo-sort')
    cargo sort --workspace --grouped {{args}}

# Get a package field from the metadata
get-crate-field field package=main_crate:  (assert-cmd 'jq')
    cargo metadata --format-version 1 | jq -e -r '.packages | map(select(.name == "{{package}}")) | first | .{{field}} // error("Field \"{{field}}\" is missing in Cargo.toml for package {{package}}")'

# Get the minimum supported Rust version (MSRV) for the crate
get-msrv package=main_crate:  (get-crate-field 'rust_version' package)

# Find the minimum supported Rust version (MSRV) using cargo-msrv extension, and update Cargo.toml
msrv:  (cargo-install 'cargo-msrv')
    cargo msrv find --write-msrv --features default -- {{just}} ci-test-msrv

# Initialize Cargo.lock file with minimal versions of dependencies.
msrv-init:  (cargo-install 'cargo-minimal-versions')
    rm -f Cargo.lock.msrv Cargo.lock
    @if ! cargo minimal-versions check --workspace ; then \
        echo "ERROR: Could not generate minimal Cargo.lock.msrv" ;\
        echo "       fix the lock file with 'cargo update ... --precise ...'" ;\
        echo "       make sure it passes 'just check' " ;\
        echo "       once done, rename Cargo.lock to Cargo.lock.msrv" ;\
        exit 1 ;\
    fi
    mv Cargo.lock Cargo.lock.msrv

# Run cargo-release
release *args='':  (cargo-install 'release-plz')
    release-plz {{args}}

# Check semver compatibility with prior published version. Install it with `cargo install cargo-semver-checks`
semver *args:  (cargo-install 'cargo-semver-checks')
    cargo semver-checks --features default {{args}}

# Run all unit and integration tests
test:
    cargo test --workspace --features default --all-targets
    cargo test --doc --workspace --features default
    @echo "--------------  Testing individual crate features"
    cargo test --workspace --all-targets --no-default-features --features aws-s3-async
    cargo test --workspace --all-targets --no-default-features --features http-async
    cargo test --workspace --all-targets --no-default-features --features iter-async
    cargo test --workspace --all-targets --no-default-features --features mmap-async-tokio
    cargo test --workspace --all-targets --no-default-features --features s3-async-native
    cargo test --workspace --all-targets --no-default-features --features s3-async-rustls
    cargo test --workspace --all-targets --no-default-features --features tilejson
    cargo test --workspace --all-targets --no-default-features --features write
    cargo test --workspace --all-targets --no-default-features --features brotli

# Test documentation generation
test-doc:  (docs '')

# Test code formatting
test-fmt:
    cargo fmt --all -- --check

# Find unused dependencies. Uses `cargo-udeps`
udeps:  (cargo-install 'cargo-udeps')
    cargo +nightly udeps --workspace --features default --all-targets

# Update all dependencies, including breaking changes. Requires nightly toolchain (install with `rustup install nightly`)
update:
    cargo +nightly -Z unstable-options update --breaking
    cargo update

# Ensure that a certain command is available
[private]
assert-cmd command:
    @if ! type {{command}} > /dev/null; then \
        echo "Command '{{command}}' could not be found. Please make sure it has been installed on your computer." ;\
        exit 1 ;\
    fi

# Make sure the git repo has no uncommitted changes
[private]
assert-git-is-clean:
    @if [ -n "$(git status --untracked-files --porcelain)" ]; then \
        >&2 echo "ERROR: git repo is no longer clean. Make sure compilation and tests artifacts are in the .gitignore, and no repo files are modified." ;\
        >&2 echo "######### git status ##########" ;\
        git status ;\
        git --no-pager diff ;\
        exit 1 ;\
    fi

# Check if a certain Cargo command is installed, and install it if needed
[private]
cargo-install $COMMAND $INSTALL_CMD='' *args='':
    #!/usr/bin/env bash
    set -euo pipefail
    if ! command -v $COMMAND > /dev/null; then
        echo "$COMMAND could not be found. Installing..."
        if ! command -v cargo-binstall > /dev/null; then
            set -x
            cargo install ${INSTALL_CMD:-$COMMAND} --locked {{args}}
            { set +x; } 2>/dev/null
        else
            set -x
            cargo binstall ${INSTALL_CMD:-$COMMAND} {{binstall_args}} --locked {{args}}
            { set +x; } 2>/dev/null
        fi
    fi
