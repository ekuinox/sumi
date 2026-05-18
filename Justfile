set windows-shell := ["powershell.exe", "-NoProfile", "-Command"]

default:
    just --list

# --- run -------------------------------------------------------------------

run *args:
    cargo run -- {{args}}

# 例: `just config path/to/config.toml` で任意の設定ファイルを使って起動。
# シェーダーは config.toml の `shader` フィールドかタスクトレイの Choose shader... で切替。
config path *args:
    cargo run -- --config {{path}} {{args}}

# --- build / examples ------------------------------------------------------

build:
    cargo build

build-release:
    cargo build --release

example name *args:
    cargo run --example {{name}} -- {{args}}

# --- installer (MSI) -------------------------------------------------------

# WiX 3.x + cargo-wix で per-user MSI を作る。出力は `target/wix/sumi-<version>-x86_64.msi`。
# 前提:
#   - WiX Toolset 3 (https://github.com/wixtoolset/wix3/releases) がインストール済み
#   - cargo-wix は mise.toml で固定済み。初回は `mise install` で取得する
#     (mise の shim が PATH に乗っていれば cargo-wix がそのまま呼べる)
# インストーラの Custom Setup ダイアログで「Launch at Windows startup」を
# チェック/外しでログオン時自動起動を切替できる。
installer:
    cargo-wix --nocapture

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
