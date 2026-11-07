#!/usr/bin/env bash
set -eo pipefail
cd "$(dirname "$0")/.."
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-features --all-targets -- -D warnings
# The tracing subscriber regression requires serial tests.
for flags in '' '--no-default-features' '--all-features' '--no-default-features --features cslb'; do
    read -r -a feature_args <<< "$flags"
    cargo test --locked -p fds "${feature_args[@]}" -- --test-threads=1
    cargo check --locked -p fds --all-targets "${feature_args[@]}"
done
cargo test --locked -p fds_cli --all-features
cargo check --locked -p fds_cli --all-targets
RUSTDOCFLAGS='-D warnings' cargo doc --locked --workspace --all-features --no-deps
cargo run --locked -p fds_cli -- --help > /dev/null
# Install with rustup toolchain install 1.86.0 --profile minimal first.
for flags in '--no-default-features' '' '--all-features'; do
    read -r -a feature_args <<< "$flags"
    cargo +1.86.0 test --locked -p fds "${feature_args[@]}" -- --test-threads=1
    cargo +1.86.0 check --locked -p fds --all-targets "${feature_args[@]}"
done
cargo +1.86.0 check --locked -p fds_cli --all-features --all-targets
python3 scripts/check_coverage.py
python3 scripts/check_docs.py
git diff --check
