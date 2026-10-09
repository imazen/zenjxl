# zenjxl justfile

# Default target
default:
    @just --list

# Run tests (default features: encode + decode)
test:
    cargo test

# Run clippy
clippy:
    cargo clippy --all-targets -- -D warnings

# Check formatting
fmt-check:
    cargo fmt --all -- --check

# Format code + regenerate the public-API surface snapshots (docs/public-api/).
# The snapshot runner lives in the standalone apidoc/ package, so it is never
# built or run by plain `cargo test` or any CI job.
fmt:
    cargo fmt --all
    cargo test --manifest-path apidoc/Cargo.toml

# Regenerate the public-API surface snapshots only
api-doc:
    cargo test --manifest-path apidoc/Cargo.toml

# Verify the committed snapshots are current
api-doc-check:
    ZEN_API_DOC=check cargo test --manifest-path apidoc/Cargo.toml

# Empirical sweep-axes + fingerprint-contract validation harness
# (docs/VARIANT_GENERATION.md §6). Writes a dated TSV under benchmarks/.
sweep-validate:
    CODEC_CORPUS_DIR="${CODEC_CORPUS_DIR:-$HOME/work/codec-eval/codec-corpus}" \
    GIT_COMMIT="$(git rev-parse --short HEAD)" \
    cargo run --release --example sweep_validate --features __expert -- \
        --out "benchmarks/sweep_validate_jxl_$(date +%F).tsv"

# Test feature permutations (requires local path deps for encode/decode)
feature-check:
    cargo check --no-default-features
    cargo check --no-default-features --features zencodec
    cargo check --no-default-features --features decode
    cargo check --no-default-features --features encode
    cargo test --no-default-features
    cargo test --no-default-features --features zencodec
    cargo test --no-default-features --features decode
    cargo test --no-default-features --features encode
    cargo test --all-features

# Clippy all feature permutations
clippy-all:
    cargo clippy --no-default-features --all-targets -- -D warnings
    cargo clippy --no-default-features --features zencodec --all-targets -- -D warnings
    cargo clippy --no-default-features --features decode --all-targets -- -D warnings
    cargo clippy --no-default-features --features encode --all-targets -- -D warnings
    cargo clippy --all-features --all-targets -- -D warnings

# Full CI check
ci: fmt-check clippy-all feature-check

# Native ARM wrapper integration coverage; includes gated alpha/lossless tests.
arm-wrapper-tests-macos:
    mkdir -p "$HOME/tmp"
    CARGO_BUILD_JOBS=4 RAYON_NUM_THREADS=4 OMP_NUM_THREADS=4 RUST_TEST_THREADS=4 TMPDIR="$HOME/tmp" nice -n 19 cargo test --locked -p zenjxl --features zencodec,__expert --lib --tests > "$HOME/tmp/zenjxl-wrapper-tests.log" 2>&1

# Structural-inventory cross-checks: every codec-corpus JXL through check_inventory,
# and the box lists against `jxlinfo -v -v` and `exiftool -v3`.
inventory-oracle:
    INVENTORY_CORPUS_JXL="${INVENTORY_CORPUS_JXL:-$HOME/work/codec-corpus/jxl}" \
    INVENTORY_ORACLE_JXLINFO="$(command -v jxlinfo)" \
    INVENTORY_ORACLE_EXIFTOOL="$(command -v exiftool)" \
    cargo test --features zencodec --test inventory corpus_ -- --nocapture
