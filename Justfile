set windows-shell := ["powershell.exe", "-NoProfile", "-Command"]

default:
    just --list

# --- run -------------------------------------------------------------------

run *args:
    cargo run -- {{args}}

run-ncs:
    cargo run -- --shader assets/ncs.wgsl

run-proximity:
    cargo run -- --shader assets/proximity.wgsl

run-proximity-still:
    cargo run -- --shader assets/proximity_still.wgsl

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
