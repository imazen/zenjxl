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

# Format code
fmt:
    cargo fmt --all

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
