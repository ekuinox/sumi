//! 設定ファイル (toml) の読み込み / デフォルト生成。
//!
//! 既定パスは `dirs::config_dir()/chryth/config.toml`
//! (Windows なら `%APPDATA%\chryth\config.toml`)。
//! ファイルが無ければ Config::default() の内容で作成し、その後はそれを読む。
//! `--config <path>` で別ファイルを指定できる。
//!
//! 初回生成時は同じディレクトリの `assets/` も scaffold される。リポジトリの
//! `assets/*.wgsl` を `include_str!` で binary に埋め込み、ユーザー環境に書き出す。
//! 既にあるファイルは上書きしないので、ユーザー編集は保持される。

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

/// binary に埋め込んでおく標準シェーダー一式。初回起動時に config と同じ
/// ディレクトリの `assets/` 配下に書き出す。
const EMBEDDED_SHADERS: &[(&str, &str)] = &[
    ("spectrum.wgsl", include_str!("../assets/spectrum.wgsl")),
    (
        "spectrum_peakhold.wgsl",
        include_str!("../assets/spectrum_peakhold.wgsl"),
    ),
    ("ring_field.wgsl", include_str!("../assets/ring_field.wgsl")),
    ("wave_rings.wgsl", include_str!("../assets/wave_rings.wgsl")),
    ("band_rings.wgsl", include_str!("../assets/band_rings.wgsl")),
];

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    /// 入力デバイスの friendly name 部分一致 (例: "Mix 3/4")
    pub device: String,
    /// 使用する WGSL シェーダーファイルのパス
    pub shader: PathBuf,
    /// 浮動小窓モードの設定
    pub floating: FloatingConfig,
    /// ログ出力の設定
    pub logging: LoggingConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct LoggingConfig {
    /// ログ出力先ディレクトリ。空文字なら config と同じディレクトリ下の `logs/`。
    pub directory: PathBuf,
    /// 何日分のログを残すか (0 で削除を無効化、起動時に古いファイルを mtime 基準で削除)
    pub retention_days: u32,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            directory: PathBuf::new(),
            retention_days: 7,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct FloatingConfig {
    /// 起動時に浮動モードで開くか
    pub enabled: bool,
    /// 全体アルファ (0-255)
    pub alpha: u8,
    /// ウィンドウ幅 (physical px)
    pub width: u32,
    /// ウィンドウ高さ (physical px)
    pub height: u32,
    /// 画面端との余白 (physical px)
    pub margin: i32,
    /// 起動時の配置位置。トレイメニューから変更したら自動保存される。
    pub placement: crate::floating::PlacementChoice,
    /// 表示するモニタの Win32 デバイス名 (例: `\\.\DISPLAY2`)。
    /// 空文字 = プライマリモニタ。トレイメニューから変更可。
    pub monitor: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            // 環境依存なので空にしておく。タスクトレイの Audio device から選んでもらう。
            device: String::new(),
            // partial config (toml に shader が無い場合) のフォールバック。
            // 実用の defaults は `Config::defaults_for(config_path)` を使う。
            shader: PathBuf::from("assets/spectrum.wgsl"),
            floating: FloatingConfig::default(),
            logging: LoggingConfig::default(),
        }
    }
}

impl Config {
    /// 初回生成時の defaults。shader と logging.directory を config と同じ
    /// ディレクトリ下に絶対パスで設定する。
    pub fn defaults_for(config_path: &Path) -> Self {
        let base = config_path.parent().unwrap_or_else(|| Path::new("."));
        Self {
            shader: base.join("assets").join("spectrum.wgsl"),
            logging: LoggingConfig {
                directory: base.join("logs"),
                retention_days: 7,
            },
            ..Self::default()
        }
    }

    /// 設定ファイル中の logging.directory が空文字 (= partial config 等)
    /// の場合は config と同じディレクトリ下の `logs/` にフォールバックする。
    pub fn resolved_log_dir(&self, config_path: &Path) -> PathBuf {
        if self.logging.directory.as_os_str().is_empty() {
            config_path
                .parent()
                .map(|d| d.join("logs"))
                .unwrap_or_else(|| PathBuf::from("logs"))
        } else {
            self.logging.directory.clone()
        }
    }
}

impl Default for FloatingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            alpha: 153,
            width: 320,
            height: 64,
            margin: 0,
            placement: crate::floating::PlacementChoice::default(),
            monitor: String::new(),
        }
    }
}

/// 既定の設定ファイルパス。
pub fn default_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("chryth")
        .join("config.toml")
}

/// 現在の Config を toml で `path` に書き出す。親ディレクトリは create_dir_all。
pub fn save(path: &Path, cfg: &Config) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("mkdir {}", parent.display()))?;
    }
    let body = toml::to_string_pretty(cfg).context("serialize config")?;
    fs::write(path, body).with_context(|| format!("write config {}", path.display()))?;
    log::info!("saved config: {}", path.display());
    Ok(())
}

/// `path` から設定を読む。ファイルが存在しない場合はデフォルト内容で
/// 自動生成 (親ディレクトリも create_dir_all で作る)。
///
/// 起動の度に config 隣の `assets/` を scaffold する (既存ファイルはスキップ
/// されるので、新しい embedded shader だけが追加される)。これによりアップデート
/// 後の起動でも新シェーダーがユーザー領域に自動で並ぶ。
pub fn load_or_create(path: &Path) -> Result<Config> {
    if let Some(parent) = path.parent() {
        scaffold_assets(&parent.join("assets"))?;
    }
    if path.exists() {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("read config {}", path.display()))?;
        let cfg: Config = toml::from_str(&raw)
            .with_context(|| format!("parse config {}", path.display()))?;
        log::info!("loaded config: {}", path.display());
        Ok(cfg)
    } else {
        let cfg = Config::defaults_for(path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("mkdir {}", parent.display()))?;
        }
        let body = toml::to_string_pretty(&cfg).context("serialize default config")?;
        fs::write(path, body)
            .with_context(|| format!("write default config {}", path.display()))?;
        log::info!("created default config: {}", path.display());
        Ok(cfg)
    }
}

/// `EMBEDDED_SHADERS` を `dir` 配下に書き出す。既存ファイルは上書きしない。
fn scaffold_assets(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("mkdir {}", dir.display()))?;
    for (name, content) in EMBEDDED_SHADERS {
        let path = dir.join(name);
        if path.exists() {
            continue;
        }
        fs::write(&path, content).with_context(|| format!("write {}", path.display()))?;
        log::info!("scaffolded shader: {}", path.display());
    }
    Ok(())
}
