//! 半透明 + クリック透過 + 常に最前面の浮動小窓を作るためのヘルパー。
//!
//! - `find_work_area_rect()`: 既定モニタの「作業領域」(画面 - タスクバー) を取得
//! - `apply_floating_styles(hwnd, alpha)`: WS_EX_LAYERED | WS_EX_TRANSPARENT |
//!   WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE を後付けで適用し、LWA_ALPHA で
//!   ウィンドウ全体を半透明にする。
//!
//! LWA_COLORKEY と違って LWA_ALPHA は compositor 段階で適用されるので、
//! wgpu の DXGI swap chain とも問題なく共存する。

use anyhow::{anyhow, Result};
use windows::Win32::Foundation::{COLORREF, HWND, RECT};
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowLongPtrW, SetLayeredWindowAttributes, SetWindowLongPtrW, SetWindowPos,
    SystemParametersInfoW, GWL_EXSTYLE, LWA_ALPHA, SPI_GETWORKAREA, SWP_FRAMECHANGED,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT,
};

#[derive(Debug, Clone, Copy)]
pub struct ScreenRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Top,
    Right,
    Bottom,
    Left,
}

/// config に保存できるユーザーの配置選択 (4 隅 + 4 辺の 8 値)。
/// シリアライズ形式は kebab-case (例: "bottom-right", "top-edge")。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PlacementChoice {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    TopEdge,
    RightEdge,
    BottomEdge,
    LeftEdge,
}

impl Default for PlacementChoice {
    fn default() -> Self {
        Self::BottomRight
    }
}

/// 配置と回転をまとめて返す。`orientation` はシェーダーに渡す回転ヒント。
#[derive(Debug, Clone, Copy)]
pub struct Placement {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    /// バー描画系シェーダー向け回転 (0=normal, 1=90°CW, 2=180°, 3=90°CCW)
    pub orientation: u32,
}

/// 4 辺のうちの 1 辺に窓を配置するための長方形を計算する。
/// `thickness` = 辺と垂直方向のウィンドウ厚 (e.g. 64px)。
/// 辺と平行方向は作業領域いっぱい (margin を考慮)。
///
/// orientation の対応は:
///   Bottom = 0 (normal, バーが下から上)
///   Right  = 1 (90°CW, バーが右から左)
///   Top    = 2 (180°, バーが上から下)
///   Left   = 3 (90°CCW, バーが左から右)
pub fn placement_at_edge(work: ScreenRect, thickness: u32, edge: Edge, margin: i32) -> Placement {
    let right = work.x + work.width as i32;
    let bottom = work.y + work.height as i32;
    let t = thickness as i32;
    let long_w = (work.width as i32 - 2 * margin).max(thickness as i32) as u32;
    let long_h = (work.height as i32 - 2 * margin).max(thickness as i32) as u32;
    match edge {
        Edge::Top => Placement {
            x: work.x + margin,
            y: work.y + margin,
            width: long_w,
            height: thickness,
            orientation: 2,
        },
        Edge::Bottom => Placement {
            x: work.x + margin,
            y: bottom - t - margin,
            width: long_w,
            height: thickness,
            orientation: 0,
        },
        Edge::Left => Placement {
            x: work.x + margin,
            y: work.y + margin,
            width: thickness,
            height: long_h,
            orientation: 3,
        },
        Edge::Right => Placement {
            x: right - t - margin,
            y: work.y + margin,
            width: thickness,
            height: long_h,
            orientation: 1,
        },
    }
}

/// 与えられた作業領域内で、指定コーナーに `win_size` のウィンドウを配置する
/// 場合の左上座標 (Physical) を返す。
pub fn position_at_corner(
    work: ScreenRect,
    win_w: u32,
    win_h: u32,
    corner: Corner,
    margin: i32,
) -> (i32, i32) {
    let right = work.x + work.width as i32;
    let bottom = work.y + work.height as i32;
    match corner {
        Corner::TopLeft => (work.x + margin, work.y + margin),
        Corner::TopRight => (right - win_w as i32 - margin, work.y + margin),
        Corner::BottomLeft => (work.x + margin, bottom - win_h as i32 - margin),
        Corner::BottomRight => (right - win_w as i32 - margin, bottom - win_h as i32 - margin),
    }
}

/// 既定モニタの作業領域 (タスクバーを除いた領域) を取得する。
pub fn find_work_area_rect() -> Option<ScreenRect> {
    unsafe {
        let mut rect = RECT::default();
        SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some(&mut rect as *mut RECT as *mut _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
        .ok()?;
        Some(ScreenRect {
            x: rect.left,
            y: rect.top,
            width: (rect.right - rect.left).max(0) as u32,
            height: (rect.bottom - rect.top).max(0) as u32,
        })
    }
}

/// 浮動窓に必要な拡張スタイルを後付けで適用する。
/// - WS_EX_LAYERED: アルファ合成を有効化
/// - WS_EX_TRANSPARENT: マウスイベントが下のウィンドウに抜ける (クリック透過)
/// - WS_EX_TOOLWINDOW: Alt+Tab / タスクバーに出さない
/// - WS_EX_NOACTIVATE: クリックしてもフォーカスを奪わない
///
/// `alpha` は 0 (完全透明) 〜 255 (不透明)。例: 60% 不透明 = 153。
pub fn apply_floating_styles(hwnd_value: isize, alpha: u8) -> Result<()> {
    if hwnd_value == 0 {
        return Err(anyhow!("invalid HWND"));
    }
    unsafe {
        let hwnd = HWND(hwnd_value);
        let current = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let new = current
            | WS_EX_LAYERED.0 as isize
            | WS_EX_TRANSPARENT.0 as isize
            | WS_EX_TOOLWINDOW.0 as isize
            | WS_EX_NOACTIVATE.0 as isize;
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, new);

        // SetWindowLong のキャッシュを commit
        SetWindowPos(
            hwnd,
            HWND(0),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        )?;

        // ウィンドウ全体のアルファを設定 (compositor 段階で適用される)
        SetLayeredWindowAttributes(hwnd, COLORREF(0), alpha, LWA_ALPHA)?;
    }
    Ok(())
}
