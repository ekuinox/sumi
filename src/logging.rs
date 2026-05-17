//! ログ初期化。
//!
//! - 出力先: 指定ディレクトリ (config.logging.directory)
//! - 形式: `chryth.log.YYYY-MM-DD` で **daily ローテーション**
//! - 保持: `tracing-appender` の `max_log_files` で古いものを自動削除
//!   (`retention_days` を保持ファイル数として渡す。0 = 削除無効)
//! - 既存の `log::*` マクロは `tracing-log::LogTracer` 経由で tracing に流れる
//! - 並行で stderr にも色付きで出す (= `cargo run` 時に普通に見える)
//!
//! 戻り値の `WorkerGuard` は non-blocking writer のワーカースレッドを生かす
//! ためのもの。プロセス終了まで drop しないこと (main で `let _g = ...` で
//! 受けるだけで OK)。

use std::fs;
use std::path::Path;

use anyhow::{Context as _, Result};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

pub fn setup(log_dir: &Path, retention_days: u32) -> Result<WorkerGuard> {
    fs::create_dir_all(log_dir).with_context(|| format!("mkdir {}", log_dir.display()))?;

    let mut builder = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("chryth.log");
    if retention_days > 0 {
        builder = builder.max_log_files(retention_days as usize);
    }
    let file_appender = builder
        .build(log_dir)
        .with_context(|| format!("create rolling appender at {}", log_dir.display()))?;
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    // RUST_LOG が無ければデフォルトで wgpu の info ノイズを落とす
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,wgpu_core=warn,wgpu_hal=warn,naga=warn"));

    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(non_blocking)
        .with_ansi(false)
        .with_target(true);

    let console_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_ansi(true)
        .with_target(false);

    tracing_subscriber::registry()
        .with(env_filter)
        .with(console_layer)
        .with(file_layer)
        .init();

    // log:: crate のレコードを tracing にブリッジ。既存の log::info!() 等が
    // そのまま tracing-subscriber 経由で stderr + file に届くようになる。
    if let Err(e) = tracing_log::LogTracer::init() {
        eprintln!("warn: tracing-log bridge failed: {e}");
    }

    tracing::info!(target: "chryth::logging", "log dir: {}", log_dir.display());
    Ok(guard)
}
