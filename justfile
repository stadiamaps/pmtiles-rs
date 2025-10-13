#!/usr/bin/env just --justfile

main_crate := 'pmtiles'
packages := '--workspace'  # All crates in the workspace
features := '--features default'
targets := '--all-targets'  # For all targets (lib, bin, tests, examples, benches)

# if running in CI, treat warnings as errors by setting RUSTFLAGS and RUSTDOCFLAGS to '-D warnings' unless they are already set
# Use `CI=true just ci-test` to run the same tests as in GitHub CI.
# Use `just env-info` to see the current values of RUSTFLAGS and RUSTDOCFLAGS
ci_mode := if env('CI', '') != '' {'1'} else {''}
# cargo-binstall needs a workaround due to caching
# ci_mode might be manually set by user, so re-check the env var
binstall_args := if env('CI', '') != '' {'--no-confirm --no-track --disable-telemetry'} else {''}
export RUSTFLAGS := env('RUSTFLAGS', if ci_mode == '1' {'-D warnings'} else {''})
export RUSTDOCFLAGS := env('RUSTDOCFLAGS', if ci_mode == '1' {'-D warnings'} else {''})
export RUST_BACKTRACE := env('RUST_BACKTRACE', if ci_mode == '1' {'1'} else {''})

@_default:
    {{just_executable()}} --list

# Build the project
build:
    cargo build {{packages}} {{features}} {{targets}}

# Quick compile without building a binary
check:
    cargo check {{packages}} {{features}} {{targets}}
    @echo "--------------  Checking individual crate features"
    cargo check {{packages}} {{targets}} --no-default-features --features aws-s3-async
    cargo check {{packages}} {{targets}} --no-default-features --features http-async
    cargo check {{packages}} {{targets}} --no-default-features --features iter-async
    cargo check {{packages}} {{targets}} --no-default-features --features mmap-async-tokio
    cargo check {{packages}} {{targets}} --no-default-features --features object-store
    cargo check {{packages}} {{targets}} --no-default-features --features s3-async-native
    cargo check {{packages}} {{targets}} --no-default-features --features s3-async-rustls
    cargo check {{packages}} {{targets}} --no-default-features --features tilejson
    cargo check {{packages}} {{targets}} --no-default-features --features write

# Generate code coverage report to upload to codecov.io
ci-coverage: env-info && \
            (coverage '--codecov --output-path target/llvm-cov/codecov.info')
    # ATTENTION: the full file path above is used in the CI workflow
    mkdir -p target/llvm-cov

# Run all tests as expected by CI
ci-test: env-info test-fmt clippy check test test-doc && assert-git-is-clean

# Run minimal subset of tests to ensure compatibility with MSRV
ci-test-msrv: env-info test

# Clean all build artifacts
clean:
    cargo clean
    rm -f Cargo.lock

# Run cargo clippy to lint the code
clippy *args:
    cargo clippy {{packages}} {{features}} {{targets}} {{args}}
    cargo clippy {{packages}} {{targets}} --no-default-features --features s3-async-native {{args}}

# Generate code coverage report. Will install `cargo llvm-cov` if missing.
coverage *args='--no-clean --open':  (cargo-install 'cargo-llvm-cov')
    cargo llvm-cov {{packages}} {{features}} {{targets}} --include-build-script {{args}}

# Build and open code documentation
docs *args='--open':
    DOCS_RS=1 cargo doc --no-deps {{args}} {{packages}} {{features}}

# Print environment info
env-info:
    @echo "Running for '{{main_crate}}' crate {{if ci_mode == '1' {'in CI mode'} else {'in dev mode'} }} on {{os()}} / {{arch()}}"
    @echo "PWD $(pwd)"
    {{just_executable()}} --version
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
    cargo sort {{packages}} --grouped {{args}}

# Get any package's field from the metadata
get-crate-field field package=main_crate:  (assert-cmd 'jq')
    cargo metadata --format-version 1 | jq -e -r '.packages | map(select(.name == "{{package}}")) | first | .{{field}} // error("Field \"{{field}}\" is missing in Cargo.toml for package {{package}}")'

# Get the minimum supported Rust version (MSRV) for the crate
get-msrv package=main_crate:  (get-crate-field 'rust_version' package)

# Find the minimum supported Rust version (MSRV) using cargo-msrv extension, and update Cargo.toml
msrv:  (cargo-install 'cargo-msrv')
    cargo msrv find --write-msrv --ignore-lockfile {{features}}

# Run cargo-release
release *args='':  (cargo-install 'release-plz')
    release-plz {{args}}

# Check semver compatibility with prior published version. Install it with `cargo install cargo-semver-checks`
semver *args:  (cargo-install 'cargo-semver-checks')
    cargo semver-checks {{features}} {{args}}

# Run all unit and integration tests
test:
    cargo test {{packages}} {{features}} {{targets}}
    cargo test --doc {{packages}} {{features}}
    @echo "--------------  Testing individual crate features"
    cargo test {{packages}} {{targets}} --no-default-features --features aws-s3-async
    cargo test {{packages}} {{targets}} --no-default-features --features http-async
    cargo test {{packages}} {{targets}} --no-default-features --features iter-async
    cargo test {{packages}} {{targets}} --no-default-features --features mmap-async-tokio
    cargo test {{packages}} {{targets}} --no-default-features --features s3-async-native
    cargo test {{packages}} {{targets}} --no-default-features --features s3-async-rustls
    cargo test {{packages}} {{targets}} --no-default-features --features tilejson
    cargo test {{packages}} {{targets}} --no-default-features --features write

# Test documentation generation
test-doc:  (docs '')

# Test code formatting
test-fmt:
    cargo fmt --all -- --check

# Find unused dependencies. Install it with `cargo install cargo-udeps`
udeps:  (cargo-install 'cargo-udeps')
    cargo +nightly udeps {{packages}} {{features}} {{targets}}

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
