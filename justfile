set shell := ["bash", "-euo", "pipefail", "-c"]

fmt:
    cargo fmt --all --check

unit:
    cargo test --locked --workspace --exclude filc-counter

clippy:
    cargo clippy --locked --workspace --all-targets -- -D warnings

e2e:
    cargo test --locked -p filc-counter

check: fmt unit clippy e2e

package:
    cargo package --locked -p filc-macros
    cargo package --locked -p filc-build
    cargo package --locked -p filc --config 'patch.crates-io.filc-macros.path="crates/filc-macros"'

smoke:
    cargo run --locked -p filc-counter
