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

# Test feature permutations (requires local path deps)
feature-check:
    cargo check --no-default-features --features encode
    cargo check --no-default-features --features decode
    cargo check --no-default-features
    cargo test --all-features

# Full CI check
ci: fmt-check clippy test feature-check
