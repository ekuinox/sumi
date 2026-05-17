//! System tray icon that shows a mini bar visualizer.
//!
//! 32x32 の RGBA バッファを毎ティック CPU で塗って `TrayIcon::set_icon` する。
//! 更新頻度はメインループ側で throttle すること (15fps 程度で十分)。
//!
//! メニューは Show / Quit の 2 項目。クリック / メニュー操作のイベントは
//! `tray_icon::TrayIconEvent::receiver()` と `tray_icon::menu::MenuEvent::receiver()`
//! でグローバルに購読できる。

use anyhow::{Context as _, Result};
use tray_icon::menu::{Menu, MenuId, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use crate::dsp::N_BARS;

pub const ICON_SIZE: u32 = 32;

pub struct Tray {
    _icon: TrayIcon,
    pub show_id: MenuId,
    pub quit_id: MenuId,
    /// 浮動モード用の「Move to」 サブメニュー: 4 隅 + 4 辺
    pub move_tl_id: MenuId,
    pub move_tr_id: MenuId,
    pub move_bl_id: MenuId,
    pub move_br_id: MenuId,
    pub edge_top_id: MenuId,
    pub edge_right_id: MenuId,
    pub edge_bottom_id: MenuId,
    pub edge_left_id: MenuId,
}

impl Tray {
    pub fn new(tooltip: &str) -> Result<Self> {
        let menu = Menu::new();
        let show = MenuItem::new("Show", true, None);

        // Move to (4 隅プリセット + 4 辺フルバー)
        let move_sub = Submenu::new("Move to", true);
        let m_tl = MenuItem::new("\u{2196}\u{FE0F} Top Left", true, None);
        let m_tr = MenuItem::new("\u{2197}\u{FE0F} Top Right", true, None);
        let m_bl = MenuItem::new("\u{2199}\u{FE0F} Bottom Left", true, None);
        let m_br = MenuItem::new("\u{2198}\u{FE0F} Bottom Right", true, None);
        let e_top = MenuItem::new("\u{2B06}\u{FE0F} Top Edge", true, None);
        let e_right = MenuItem::new("\u{27A1}\u{FE0F} Right Edge", true, None);
        let e_bottom = MenuItem::new("\u{2B07}\u{FE0F} Bottom Edge", true, None);
        let e_left = MenuItem::new("\u{2B05}\u{FE0F} Left Edge", true, None);
        move_sub.append(&m_tl).context("append Move TL")?;
        move_sub.append(&m_tr).context("append Move TR")?;
        move_sub.append(&m_bl).context("append Move BL")?;
        move_sub.append(&m_br).context("append Move BR")?;
        move_sub
            .append(&PredefinedMenuItem::separator())
            .context("append separator in Move to")?;
        move_sub.append(&e_top).context("append Edge Top")?;
        move_sub.append(&e_right).context("append Edge Right")?;
        move_sub.append(&e_bottom).context("append Edge Bottom")?;
        move_sub.append(&e_left).context("append Edge Left")?;

        let quit = MenuItem::new("Quit", true, None);

        menu.append(&show).context("append Show menu item")?;
        menu.append(&move_sub).context("append Move to submenu")?;
        menu.append(&PredefinedMenuItem::separator())
            .context("append separator")?;
        menu.append(&quit).context("append Quit menu item")?;

        let show_id = show.id().clone();
        let quit_id = quit.id().clone();
        let move_tl_id = m_tl.id().clone();
        let move_tr_id = m_tr.id().clone();
        let move_bl_id = m_bl.id().clone();
        let move_br_id = m_br.id().clone();
        let edge_top_id = e_top.id().clone();
        let edge_right_id = e_right.id().clone();
        let edge_bottom_id = e_bottom.id().clone();
        let edge_left_id = e_left.id().clone();

        let initial = render_icon_rgba(&[0.0; N_BARS]);
        let icon = Icon::from_rgba(initial, ICON_SIZE, ICON_SIZE)
            .context("create initial tray icon")?;

        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip(tooltip)
            .with_icon(icon)
            .build()
            .context("build tray icon")?;

        Ok(Self {
            _icon: tray,
            show_id,
            quit_id,
            move_tl_id,
            move_tr_id,
            move_bl_id,
            move_br_id,
            edge_top_id,
            edge_right_id,
            edge_bottom_id,
            edge_left_id,
        })
    }

    pub fn update_from_bars(&self, bars: &[f32; N_BARS]) {
        let bytes = render_icon_rgba(bars);
        if let Ok(icon) = Icon::from_rgba(bytes, ICON_SIZE, ICON_SIZE) {
            // 失敗は致命ではないので無視
            let _ = self._icon.set_icon(Some(icon));
        }
    }
}

/// `N_BARS` の値を 32 列に集約して RGBA バッファに描画する。
/// 戻り値は ICON_SIZE * ICON_SIZE * 4 バイトの行優先 RGBA。
pub fn render_icon_rgba(bars: &[f32; N_BARS]) -> Vec<u8> {
    const SIZE: usize = ICON_SIZE as usize;
    let mut rgba = vec![0u8; SIZE * SIZE * 4];

    // 背景: 半透明の暗色を塗っておく (無音でもアイコンの輪郭が見える)
    for px in 0..SIZE * SIZE {
        let idx = px * 4;
        rgba[idx + 0] = 25;
        rgba[idx + 1] = 25;
        rgba[idx + 2] = 35;
        rgba[idx + 3] = 180; // 完全不透明ではなく半透明
    }

    // 32 列、それぞれが (N_BARS / 32) 個の bar の max を担当
    const STRIDE: usize = N_BARS / SIZE; // 96 / 32 = 3
    for x in 0..SIZE {
        let base = x * STRIDE;
        let mut amp = 0.0_f32;
        for i in 0..STRIDE {
            amp = amp.max(bars[base + i]);
        }
        // 最低でも 1px は塗る (= 無音でも底ラインが見える)
        let bar_height = ((amp.clamp(0.0, 1.0) * SIZE as f32).round() as usize).max(1);
        for y in 0..bar_height.min(SIZE) {
            // y=0 が画面下にしたいので逆向き
            let row = SIZE - 1 - y;
            let pixel = row * SIZE + x;
            let idx = pixel * 4;
            // VU メーター配色: 下=緑 → 中=黄 → 上=赤
            let (r, g, b) = vu_color(y as f32 / (SIZE as f32 - 1.0));
            rgba[idx + 0] = r;
            rgba[idx + 1] = g;
            rgba[idx + 2] = b;
            rgba[idx + 3] = 255;
        }
    }

    rgba
}

fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    let v = a as f32 + (b as f32 - a as f32) * t;
    v.clamp(0.0, 255.0) as u8
}

/// VU メーター式の色マップ。t=0 で緑、t=0.6 で黄、t=1.0 で赤。
fn vu_color(t: f32) -> (u8, u8, u8) {
    let t = t.clamp(0.0, 1.0);
    if t < 0.6 {
        // 緑 (60, 220, 60) → 黄 (240, 220, 50)
        let s = t / 0.6;
        (lerp_u8(60, 240, s), 220, lerp_u8(60, 50, s))
    } else {
        // 黄 (240, 220, 50) → 赤 (240, 50, 50)
        let s = (t - 0.6) / 0.4;
        (240, lerp_u8(220, 50, s), 50)
    }
}
