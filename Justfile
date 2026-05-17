set windows-shell := ["powershell.exe", "-NoProfile", "-Command"]

default:
    just --list

# --- run -------------------------------------------------------------------

run *args:
    cargo run -- {{args}}

# 例: `just shader assets/foo.wgsl` で任意のシェーダーを使って起動
shader path *args:
    cargo run -- --shader {{path}} {{args}}

# --- build / examples ------------------------------------------------------

build:
    cargo build

build-release:
    cargo build --release

example name *args:
    cargo run --example {{name}} -- {{args}}

# --- checks ----------------------------------------------------------------

fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all -- --check

lint:
    cargo clippy --all-targets --all-features -- -D warnings

test:
    cargo test --all

check:
    cargo check --all-targets --all-features

validate: fmt-check lint test
