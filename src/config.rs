//! 設定ファイル (toml) の読み込み / デフォルト生成。
//!
//! 既定パスは `dirs::config_dir()/chryth/config.toml`
//! (Windows なら `%APPDATA%\chryth\config.toml`)。
//! ファイルが無ければ Config::default() の内容で作成し、その後はそれを読む。
//! `--config <path>` で別ファイルを指定できる。

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    /// 入力デバイスの friendly name 部分一致 (例: "Mix 3/4")
    pub device: String,
    /// 使用する WGSL シェーダーファイルのパス
    pub shader: PathBuf,
    /// 浮動小窓モードの設定
    pub floating: FloatingConfig,
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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            device: "Mix 3/4".into(),
            shader: PathBuf::from("assets/spectrum.wgsl"),
            floating: FloatingConfig::default(),
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

/// `path` から設定を読む。ファイルが存在しない場合はデフォルト内容で
/// 自動生成 (親ディレクトリも create_dir_all で作る)。
pub fn load_or_create(path: &Path) -> Result<Config> {
    if path.exists() {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("read config {}", path.display()))?;
        let cfg: Config = toml::from_str(&raw)
            .with_context(|| format!("parse config {}", path.display()))?;
        log::info!("loaded config: {}", path.display());
        Ok(cfg)
    } else {
        let cfg = Config::default();
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
