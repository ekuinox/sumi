//! ビルド時にコミットハッシュ / ビルド時刻を埋め込んで実行時に取り出すユーティリティ。
//!
//! 埋め込みは `build.rs` 経由 (`cargo:rustc-env`)。値が無い場合は "unknown" に倒す。
//! ハッシュは `SUMI_COMMIT_HASH` → `GITHUB_SHA` → `git rev-parse --short HEAD` の順に解決。
//! 詳細は `build.rs` のコメント参照。

/// `Cargo.toml` の version。
pub fn app_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// ビルド時のコミットハッシュ (短縮 7 文字)。
pub fn commit_hash() -> &'static str {
    option_env!("SUMI_COMMIT_HASH").unwrap_or("unknown")
}

/// ビルド時刻 (UTC ISO 8601、秒精度)。
pub fn build_timestamp() -> &'static str {
    option_env!("SUMI_BUILD_TIMESTAMP").unwrap_or("unknown")
}

/// About ダイアログに出す複数行サマリ。
pub fn summary(config_path: &std::path::Path) -> String {
    format!(
        "sumi {version}\n\
         commit:  {hash}\n\
         built:   {built}\n\
         config:  {config}",
        version = app_version(),
        hash = commit_hash(),
        built = build_timestamp(),
        config = config_path.display(),
    )
}
