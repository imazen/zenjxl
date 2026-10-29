# zenjxl justfile

# Default target
default:
    @just --list

# Export only this package and its standalone consumer, away from workspace patches.
collection-validation-sync destination:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p "{{destination}}/src" "{{destination}}/examples" "{{destination}}/validation/collections"
    cp Cargo.toml "{{destination}}/"
    rsync -a src/ "{{destination}}/src/"
    rsync -a examples/ "{{destination}}/examples/"
    rsync -a --exclude target validation/collections/ "{{destination}}/validation/collections/"

collection-validation:
    cargo test --manifest-path validation/collections/Cargo.toml
    cargo clippy --manifest-path validation/collections/Cargo.toml --all-targets -- -D warnings
    cargo clippy --manifest-path validation/collections/Cargo.toml -p zenjxl --lib -- -D warnings

collection-portability:
    cargo check --manifest-path validation/collections/Cargo.toml --lib --target wasm32-wasip1
    cargo check --manifest-path validation/collections/Cargo.toml --lib --target i686-unknown-linux-gnu

collection-i686-validation:
    cargo test --manifest-path validation/collections/Cargo.toml --target i686-unknown-linux-gnu --test consumer

# Caller supplies a disk-backed scratch and corpus cache; no sibling source mounts.
collection-cross-validation:
    cd validation/collections && CROSS_CONFIG="$PWD/Cross.toml" cross test --target i686-unknown-linux-gnu --test consumer

collection-doc:
    RUSTDOCFLAGS="-D warnings" cargo doc --manifest-path validation/collections/Cargo.toml --no-deps -p zenjxl

collection-owner-contract-check:
    just collection-portability
    just collection-doc
    just collection-api docs/public-api/zenjxl.collections.txt
    cargo fmt --manifest-path validation/collections/Cargo.toml -p zenjxl-collection-validation -- --check

collection-context-check:
    rustfmt --edition 2024 src/collection/mod.rs
    cargo fmt --manifest-path validation/collections/Cargo.toml -p zenjxl-collection-validation
    just collection-validation
    just collection-owner-contract-check

# Source-derived review surface for the optional collection feature.
collection-api output:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo public-api --manifest-path validation/collections/Cargo.toml -p zenjxl -sss --color never | rg 'zenjxl::collection' > "{{output}}"

# Refresh our root build export without reading live sibling worktrees.
# Caller prepares immutable dependency checkouts at the manifest's paths.
integration-validation-sync destination:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p "{{destination}}"
    rsync -a --exclude .git --exclude .jj --exclude .workongoing --exclude target --exclude .zencodec --exclude Cargo.lock ./ "{{destination}}/"

integration-validation:
    just integration-check
    cargo test --all-features --test gain_map_render --test readme_examples --test lossless_verify --test alpha_axes_class_conditional --test jpeg_lossy

integration-check:
    just contract-identity
    cargo clippy --all-features --all-targets -- -D warnings

contract-identity:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo metadata --format-version 1 --all-features | python3 scripts/check-contract-graph.py

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

# Metadata integration; resource limits are supplied by the caller.
metadata-check:
    cd .. && cargo test --manifest-path zenjxl/Cargo.toml --features inventory --test inventory --test inventory_alloc --test fuzz_regression --test inventory_feature
    cd .. && cargo clippy --manifest-path zenjxl/Cargo.toml --features inventory --all-targets -- -D warnings
    cd .. && cargo test --manifest-path zenjxl/Cargo.toml --no-default-features --features decode,zencodec --test inventory_feature

inventory-oracle:
    cd .. && INVENTORY_CORPUS_JXL="${INVENTORY_CORPUS_JXL:-$HOME/work/codec-corpus/jxl}" INVENTORY_ORACLE_JXLINFO="$(command -v jxlinfo)" INVENTORY_ORACLE_EXIFTOOL="$(command -v exiftool)" cargo test --manifest-path zenjxl/Cargo.toml --features inventory --test inventory corpus_ -- --nocapture

metadata-portability:
    cd .. && CARGO_TARGET_I686_UNKNOWN_LINUX_GNU_RUNNER=env cargo test --manifest-path zenjxl/Cargo.toml --no-default-features --features inventory --test inventory_feature --test fuzz_regression --target i686-unknown-linux-gnu
    cd .. && cargo test --manifest-path zenjxl/apidoc/Cargo.toml

metadata-semver:
    #!/usr/bin/env bash
    set -euo pipefail
    root="{{justfile_directory()}}"
    baseline=$(cargo read zenjxl==0.2.1 --path-only)
    cd "$root/.."
    RUSTC_BOOTSTRAP=1 cargo rustdoc --manifest-path "$root/Cargo.toml" --lib --features inventory,reconstruct-hdr,jpeg-lossy -- -Zunstable-options --output-format json
    RUSTC_BOOTSTRAP=1 cargo rustdoc --manifest-path "$baseline/Cargo.toml" --target-dir "$root/target/published-api/0.2.1" --lib --features zencodec -- -Zunstable-options --output-format json
    cargo semver-checks check-release --manifest-path "$root/Cargo.toml" --current-rustdoc "$root/target/doc/zenjxl.json" --baseline-rustdoc "$root/target/published-api/0.2.1/doc/zenjxl.json" --release-type patch

metadata-fuzz-check:
    cd .. && cargo check --manifest-path zenjxl/fuzz/Cargo.toml --bin inventory

# Codec-owned planner, with neither encoder nor decoder enabled.
metadata-rewrite-check:
    cd .. && cargo test --manifest-path zenjxl/Cargo.toml --no-default-features --features metadata-rewrite --test metadata_rewrite
    cd .. && cargo clippy --manifest-path zenjxl/Cargo.toml --no-default-features --features metadata-rewrite --lib -- -D warnings
    cd .. && CARGO_TARGET_I686_UNKNOWN_LINUX_GNU_RUNNER=env cargo test --manifest-path zenjxl/Cargo.toml --no-default-features --features metadata-rewrite --test metadata_rewrite --target i686-unknown-linux-gnu

metadata-rewrite-oracle:
    #!/usr/bin/env bash
    set -euo pipefail
    root="$PWD"
    output="$HOME/tmp/zenmeta-jxl-rewrite-oracle"
    cd ..
    cargo run --manifest-path "$root/Cargo.toml" --features metadata-rewrite --example metadata_rewrite_oracle -- "$output"
    djxl "$output/before.jxl" "$output/before.ppm"
    djxl "$output/after.jxl" "$output/after.ppm"
    cmp "$output/before.ppm" "$output/after.ppm"
    jxlinfo -v "$output/after.jxl"

# Shared timing admission plus encoded-duration roundtrip.
animation-timing-check:
    cargo test -p zenjxl --features zencodec --test animation_timing
    cargo clippy -p zenjxl --features zencodec --lib --test animation_timing -- -D warnings

animation-timing-oracle output:
    cargo run -p zenjxl --features zencodec --example animation_timing_oracle -- "{{output}}"
    jxlinfo -v "{{output}}"
    djxl "{{output}}" "{{output}}.png"

# Shared contract suite with inventory enabled and absent.
shared-contract-check:
    cargo fmt -p zenjxl
    cargo clippy -p zenjxl --features zencodec --test shared_contracts -- -D warnings
    cargo test -p zenjxl --features zencodec,inventory,metadata-rewrite --test shared_contracts
    cargo test -p zenjxl --features zencodec,metadata-rewrite --test shared_contracts
    cargo clippy -p zenjxl --features zencodec,inventory,metadata-rewrite --test shared_contracts -- -D warnings

# Shared metadata planners and inventory dispatch on the consumer's exact revision.
contract-pin-check:
    just shared-contract-check
    cargo test --no-default-features --features metadata-rewrite --test metadata_rewrite
    just metadata-fuzz-check

# CICP headers and native sample equality for the owner-selected intent policy.
rendering-intent-check:
    cargo test --locked --lib --features zencodec cicp_p3_and_pq_headers_use_relative_intent
    cargo test --locked --lib --features zencodec metadata_cicp_round_trips_via_enum_color
    cargo clippy --locked --lib --features zencodec -- -D warnings

# Exercise gain-map reconstruction against the pinned current ultrahdr source.
hdr-reconstruction-check:
    cargo test --locked --features reconstruct-hdr --test gain_map_render
    cargo clippy --locked --features reconstruct-hdr --lib -- -D warnings

# Isolated root: optional Pulse, native metadata and collection feature integration.
release-contract-check:
    cargo metadata --locked --all-features --format-version 1 | python3 scripts/check-contract-graph.py
    cargo test --locked --features zencodec,metadata-rewrite,collections --test shared_contracts --test metadata_rewrite
    cargo clippy --locked --features zencodec,metadata-rewrite,collections --lib --test shared_contracts --test metadata_rewrite -- -D warnings

release-feature-check:
    cargo check --all-features --all-targets --locked

release-runtime-check:
    cargo test --all-features --locked

release-lint-check:
    cargo clippy --all-features --all-targets --locked -- -D warnings

# Declared row protocol, padded parity and existing encode metadata controls.
declared-rows-check:
    cargo test --locked --features zencodec --test declared_rows --test encode_color_signal --test shared_contracts
    cargo clippy --locked --features zencodec --lib --test declared_rows -- -D warnings
