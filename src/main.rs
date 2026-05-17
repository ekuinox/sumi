mod audio;
mod config;
mod dsp;
mod floating;
mod render;
mod tray;

use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use clap::Parser;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::{Window, WindowAttributes, WindowId, WindowLevel};

use crate::audio::{spawn_capture, CaptureHandle};
use crate::dsp::{Dsp, FFT_SIZE, N_BARS};
use crate::render::{Renderer, WAVE_LEN};
use crate::tray::Tray;

#[derive(Parser, Debug)]
struct Cli {
    /// 設定ファイル (toml) のパス。省略時は `dirs::config_dir()/chryth/config.toml`
    /// (Windows なら `%APPDATA%\chryth\config.toml`)。ファイルが無ければデフォルト
    /// 内容で自動生成して開く。
    #[clap(long)]
    config: Option<PathBuf>,
}

fn main() -> Result<()> {
    if std::env::var_os("RUST_LOG").is_none() {
        std::env::set_var("RUST_LOG", "info,wgpu_core=warn,wgpu_hal=warn,naga=warn");
    }
    env_logger::init();
    let cli = Cli::parse();

    let config_path = cli.config.unwrap_or_else(config::default_path);
    let cfg = config::load_or_create(&config_path)?;

    let shader_path = cfg
        .shader
        .canonicalize()
        .with_context(|| format!("shader file not found: {}", cfg.shader.display()))?;
    let initial_shader = std::fs::read_to_string(&shader_path)
        .with_context(|| format!("read shader {}", shader_path.display()))?;

    let capture = spawn_capture(cfg.device.clone(), 1 << 16)?;
    log::info!(
        "audio format: {} Hz, {} ch, {} bit, float={}",
        capture.format.sample_rate,
        capture.format.channels,
        capture.format.bits_per_sample,
        capture.format.is_float
    );

    let (watch_tx, watch_rx) = mpsc::channel::<notify::Result<notify::Event>>();
    let mut watcher = notify::recommended_watcher(move |res| {
        let _ = watch_tx.send(res);
    })
    .context("create file watcher")?;
    watcher
        .watch(&shader_path, RecursiveMode::NonRecursive)
        .with_context(|| format!("watch shader {}", shader_path.display()))?;

    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = App {
        capture,
        window: None,
        renderer: None,
        dsp: None,
        scratch: vec![0.0; FFT_SIZE].into_boxed_slice(),
        shader_path,
        initial_shader,
        watch_rx,
        _watcher: watcher,
        last_reload: Instant::now() - Duration::from_secs(1),
        tray: None,
        last_tray_update: Instant::now() - Duration::from_secs(1),
        latest_bars: Box::new([0.0; N_BARS]),
        floating: cfg.floating.enabled,
        floating_alpha: cfg.floating.alpha,
        floating_w: cfg.floating.width,
        floating_h: cfg.floating.height,
        floating_margin: cfg.floating.margin,
        cfg,
        config_path,
    };
    event_loop.run_app(&mut app)?;
    Ok(())
}

struct App {
    capture: CaptureHandle,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    dsp: Option<Dsp>,
    scratch: Box<[f32]>,
    shader_path: PathBuf,
    initial_shader: String,
    watch_rx: mpsc::Receiver<notify::Result<notify::Event>>,
    _watcher: RecommendedWatcher,
    last_reload: Instant,
    tray: Option<Tray>,
    last_tray_update: Instant,
    /// 直近のバー値。RedrawRequested で計算後にここに保管し、about_to_wait で
    /// throttle してトレイへ反映する。
    latest_bars: Box<[f32; N_BARS]>,
    /// true なら半透明クリック透過の浮動小窓で起動する
    floating: bool,
    /// 浮動窓モードでのウィンドウ全体のアルファ (0-255)
    floating_alpha: u8,
    /// 浮動窓モードのウィンドウサイズ (physical px)
    floating_w: u32,
    floating_h: u32,
    /// 画面端との余白 (physical px)
    floating_margin: i32,
    /// 現在の Config (トレイからの変更を反映してファイルに書き戻す)
    cfg: config::Config,
    /// Config の書き戻し先パス
    config_path: PathBuf,
}

impl App {
    fn restore_window(&self) {
        if let Some(w) = &self.window {
            w.set_minimized(false);
            w.set_visible(true);
            // サイズは触らない (OS が元のサイズを保持する。明示リサイズすると
            // floating モードでは 320x64 が 960x540 等に化けるバグの原因になる)
            w.focus_window();
        }
    }

    fn minimize_window(&self) {
        if let Some(w) = &self.window {
            w.set_minimized(true);
        }
    }

    fn drain_tray_events(&mut self, event_loop: &ActiveEventLoop) {
        let Some(tray) = &self.tray else {
            return;
        };
        // tray icon 本体のクリック等
        while let Ok(ev) = tray_icon::TrayIconEvent::receiver().try_recv() {
            if matches!(
                ev,
                tray_icon::TrayIconEvent::DoubleClick { .. }
                    | tray_icon::TrayIconEvent::Click {
                        button: tray_icon::MouseButton::Left,
                        button_state: tray_icon::MouseButtonState::Up,
                        ..
                    }
            ) {
                self.restore_window();
            }
        }
        // メニュークリック処理。借用の都合で MenuId からアクションを抽出し、
        // tray の借用が切れた後で self を mut で触る。
        use floating::PlacementChoice as P;
        let mut choice_selected: Option<P> = None;
        let mut device_selected: Option<String> = None;
        let mut monitor_selected: Option<String> = None;
        let mut floating_toggle_requested = false;
        while let Ok(ev) = tray_icon::menu::MenuEvent::receiver().try_recv() {
            if ev.id == tray.show_id {
                self.restore_window();
            } else if ev.id == tray.minimize_id {
                self.minimize_window();
            } else if ev.id == tray.floating_toggle_id {
                floating_toggle_requested = true;
            } else if ev.id == tray.restart_id {
                log::info!("restart requested via tray");
                spawn_restart();
                event_loop.exit();
            } else if ev.id == tray.quit_id {
                event_loop.exit();
            } else if ev.id == tray.move_tl_id {
                choice_selected = Some(P::TopLeft);
            } else if ev.id == tray.move_tr_id {
                choice_selected = Some(P::TopRight);
            } else if ev.id == tray.move_bl_id {
                choice_selected = Some(P::BottomLeft);
            } else if ev.id == tray.move_br_id {
                choice_selected = Some(P::BottomRight);
            } else if ev.id == tray.edge_top_id {
                choice_selected = Some(P::TopEdge);
            } else if ev.id == tray.edge_right_id {
                choice_selected = Some(P::RightEdge);
            } else if ev.id == tray.edge_bottom_id {
                choice_selected = Some(P::BottomEdge);
            } else if ev.id == tray.edge_left_id {
                choice_selected = Some(P::LeftEdge);
            } else if let Some((_, name)) =
                tray.devices.iter().find(|(id, _)| *id == ev.id)
            {
                device_selected = Some(name.clone());
            } else if let Some((_, name)) =
                tray.monitors.iter().find(|(id, _)| *id == ev.id)
            {
                monitor_selected = Some(name.clone());
            }
        }
        if let Some(choice) = choice_selected {
            self.apply_choice(choice);
        }
        if let Some(name) = device_selected {
            self.select_device(name, event_loop);
        }
        if let Some(name) = monitor_selected {
            self.select_monitor(name, event_loop);
        }
        if floating_toggle_requested {
            self.toggle_floating(event_loop);
        }
    }

    fn select_monitor(&mut self, name: String, event_loop: &ActiveEventLoop) {
        log::info!("monitor selected: {:?}", name);
        self.cfg.floating.monitor = name;
        if let Err(e) = config::save(&self.config_path, &self.cfg) {
            log::warn!("config save failed: {e:#}");
            return;
        }
        log::info!("restarting chryth to apply new monitor...");
        spawn_restart();
        event_loop.exit();
    }

    /// 配置選択をウィンドウへ反映し、選択値を config に保存する。
    fn apply_choice(&mut self, choice: floating::PlacementChoice) {
        let p = self.placement_for_choice(choice);
        self.apply_placement(p);
        self.cfg.floating.placement = choice;
        if let Err(e) = config::save(&self.config_path, &self.cfg) {
            log::warn!("config save failed: {e:#}");
        }
    }

    fn toggle_floating(&mut self, event_loop: &ActiveEventLoop) {
        self.cfg.floating.enabled = !self.cfg.floating.enabled;
        log::info!("floating toggled to {}", self.cfg.floating.enabled);
        if let Err(e) = config::save(&self.config_path, &self.cfg) {
            log::warn!("config save failed: {e:#}");
            return;
        }
        log::info!("restarting chryth to apply floating mode change...");
        spawn_restart();
        event_loop.exit();
    }

    fn select_device(&mut self, name: String, event_loop: &ActiveEventLoop) {
        log::info!("device selected: {name}");
        self.cfg.device = name;
        if let Err(e) = config::save(&self.config_path, &self.cfg) {
            log::warn!("config save failed: {e:#}");
            return;
        }
        log::info!("restarting chryth to apply new device...");
        spawn_restart();
        event_loop.exit();
    }

    fn work_area(&self) -> floating::ScreenRect {
        floating::find_work_area_for_monitor(&self.cfg.floating.monitor).unwrap_or(
            floating::ScreenRect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1032,
            },
        )
    }

    fn corner_placement(&self, corner: floating::Corner) -> floating::Placement {
        let work = self.work_area();
        let (x, y) = floating::position_at_corner(
            work,
            self.floating_w,
            self.floating_h,
            corner,
            self.floating_margin,
        );
        floating::Placement {
            x,
            y,
            width: self.floating_w,
            height: self.floating_h,
            orientation: 0,
        }
    }

    fn edge_placement(&self, edge: floating::Edge) -> floating::Placement {
        let work = self.work_area();
        // 辺フルバーの「厚み」には floating_height を使う
        floating::placement_at_edge(work, self.floating_h, edge, self.floating_margin)
    }

    /// 設定保存される PlacementChoice → 実 Placement の変換
    fn placement_for_choice(&self, choice: floating::PlacementChoice) -> floating::Placement {
        use floating::{Corner, Edge, PlacementChoice as P};
        match choice {
            P::TopLeft => self.corner_placement(Corner::TopLeft),
            P::TopRight => self.corner_placement(Corner::TopRight),
            P::BottomLeft => self.corner_placement(Corner::BottomLeft),
            P::BottomRight => self.corner_placement(Corner::BottomRight),
            P::TopEdge => self.edge_placement(Edge::Top),
            P::RightEdge => self.edge_placement(Edge::Right),
            P::BottomEdge => self.edge_placement(Edge::Bottom),
            P::LeftEdge => self.edge_placement(Edge::Left),
        }
    }

    fn apply_placement(&mut self, p: floating::Placement) {
        let Some(win) = self.window.clone() else {
            return;
        };
        let _ = win.request_inner_size(PhysicalSize::new(p.width, p.height));
        win.set_outer_position(PhysicalPosition::new(p.x, p.y));
        if let Some(r) = self.renderer.as_mut() {
            r.set_orientation(p.orientation);
        }
        log::info!(
            "placed window: {}x{} @ ({},{}) orientation={}",
            p.width,
            p.height,
            p.x,
            p.y,
            p.orientation
        );
    }

    fn tick_tray_icon(&mut self) {
        let Some(tray) = &self.tray else {
            return;
        };
        // ~15 fps に絞る
        if self.last_tray_update.elapsed() < Duration::from_millis(66) {
            return;
        }
        tray.update_from_bars(&self.latest_bars);
        self.last_tray_update = Instant::now();
    }

    fn drain_shader_events_and_reload(&mut self) {
        let mut should_reload = false;
        while let Ok(res) = self.watch_rx.try_recv() {
            match res {
                Ok(_) => should_reload = true,
                Err(e) => log::warn!("shader watcher error: {e}"),
            }
        }
        if !should_reload {
            return;
        }
        // Debounce: editors often emit multiple events per save (rename + create etc).
        // Coalesce within a small window.
        if self.last_reload.elapsed() < Duration::from_millis(100) {
            return;
        }
        let Some(renderer) = &mut self.renderer else {
            return;
        };
        let source = match std::fs::read_to_string(&self.shader_path) {
            Ok(s) => s,
            Err(e) => {
                // file may be momentarily missing during atomic-save; retry once shortly
                std::thread::sleep(Duration::from_millis(20));
                match std::fs::read_to_string(&self.shader_path) {
                    Ok(s) => s,
                    Err(_) => {
                        log::warn!("shader read failed: {e}");
                        return;
                    }
                }
            }
        };
        match renderer.reload_shader(&source) {
            Ok(()) => log::info!("shader reloaded: {}", self.shader_path.display()),
            Err(e) => log::warn!("shader reload failed:\n{e}"),
        }
        self.last_reload = Instant::now();
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let initial_placement = if self.floating {
            Some(self.placement_for_choice(self.cfg.floating.placement))
        } else {
            None
        };

        let attrs = if let Some(p) = initial_placement {
            log::info!(
                "floating placement {:?}: {}x{} @ ({},{}) orientation={}",
                self.cfg.floating.placement,
                p.width,
                p.height,
                p.x,
                p.y,
                p.orientation
            );
            WindowAttributes::default()
                .with_title("chryth")
                .with_decorations(false)
                .with_resizable(false)
                .with_window_level(WindowLevel::AlwaysOnTop)
                .with_position(PhysicalPosition::new(p.x, p.y))
                .with_inner_size(PhysicalSize::new(p.width, p.height))
        } else {
            WindowAttributes::default()
                .with_title("chryth")
                .with_inner_size(LogicalSize::new(960.0, 360.0))
        };

        let window = event_loop.create_window(attrs).expect("create window");
        let window = Arc::new(window);

        // 浮動モード時は LWA_ALPHA + click-through + tool-window を後付け
        if self.floating {
            if let Ok(handle) = window.window_handle() {
                if let RawWindowHandle::Win32(h) = handle.as_raw() {
                    if let Err(e) =
                        floating::apply_floating_styles(h.hwnd.get(), self.floating_alpha)
                    {
                        log::warn!("apply_floating_styles failed: {e:#}");
                    }
                }
            }
            // スタイル変更で SetWindowPos が走ったあと、サイズが initial size と
            // ズレるケースがあるので明示的に揃え直す。Edge 配置の場合は 320x64
            // ではなく placement のサイズを使う必要がある (= 縦長 / 横長で値が違う)。
            if let Some(p) = initial_placement {
                let _ = window.request_inner_size(PhysicalSize::new(p.width, p.height));
            }
            log::info!(
                "floating actual outer_size after styles: {:?}",
                window.outer_size()
            );
        }

        let mut renderer = pollster::block_on(Renderer::new(window.clone(), &self.initial_shader))
            .expect("renderer");
        // 起動時の配置に対応する orientation も適用
        if let Some(p) = initial_placement {
            renderer.set_orientation(p.orientation);
        }
        let dsp = Dsp::new(self.capture.format.sample_rate);

        // Tray icon は同じスレッド (= winit event loop = main thread) で作る必要がある。
        // 入力デバイス一覧もここで列挙して Audio device サブメニューに流し込む。
        let devices = match audio::list_capture_devices() {
            Ok(v) => v,
            Err(e) => {
                log::warn!("device enumeration failed: {e:#}");
                Vec::new()
            }
        };
        let monitors = floating::list_monitors();
        log::info!("found {} monitor(s)", monitors.len());
        for m in &monitors {
            log::info!(
                "  monitor {}: work {}x{} @ ({},{}) primary={}",
                m.device_name,
                m.work.width,
                m.work.height,
                m.work.x,
                m.work.y,
                m.is_primary
            );
        }
        match Tray::new(
            "chryth",
            &devices,
            &self.cfg.device,
            self.cfg.floating.enabled,
            &monitors,
            &self.cfg.floating.monitor,
        ) {
            Ok(t) => self.tray = Some(t),
            Err(e) => log::warn!("tray icon disabled: {e:#}"),
        }

        self.window = Some(window);
        self.renderer = Some(renderer);
        self.dsp = Some(dsp);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(r) = &mut self.renderer {
                    r.resize(size);
                }
            }
            WindowEvent::RedrawRequested => {
                let (Some(renderer), Some(dsp)) = (&mut self.renderer, &mut self.dsp) else {
                    return;
                };
                let samples_slice: &mut [f32; FFT_SIZE] =
                    (&mut self.scratch[..FFT_SIZE]).try_into().unwrap();
                self.capture.samples.read_latest(samples_slice);
                let bars = dsp.process(samples_slice);

                // 直近 (WAVE_LEN * WAVE_STRIDE) サンプルを WAVE_LEN 点に平均ダウンサンプル。
                // 16x 平均 ≒ ~1.4kHz の lowpass で、波形がガタつかずに滑らかに見える。
                // 大きくし過ぎるとベースしか見えなくなる、小さくし過ぎると形が荒れる。
                const WAVE_STRIDE: usize = 16;
                let raw_start = FFT_SIZE - WAVE_LEN * WAVE_STRIDE;
                let mut wave: [f32; WAVE_LEN] = [0.0; WAVE_LEN];
                for (i, slot) in wave.iter_mut().enumerate() {
                    let base = raw_start + i * WAVE_STRIDE;
                    let mut sum = 0.0;
                    for s in &samples_slice[base..base + WAVE_STRIDE] {
                        sum += *s;
                    }
                    *slot = sum / WAVE_STRIDE as f32;
                }

                renderer.update(bars, &wave);

                // about_to_wait 側でトレイ更新したいので最新値をコピーしておく
                self.latest_bars.copy_from_slice(bars);

                match renderer.render() {
                    Ok(_) => {}
                    Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                        renderer.resize(renderer.size);
                    }
                    Err(e) => log::error!("render error: {e:?}"),
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.drain_shader_events_and_reload();
        self.drain_tray_events(event_loop);
        self.tick_tray_icon();
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }
}

const _: () = assert!(N_BARS == 96);

/// 自プロセスを同じ CLI 引数で再起動する。新プロセスを spawn したら呼び出し側で
/// event_loop.exit() してこのプロセスを畳む想定。
fn spawn_restart() {
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => {
            log::error!("current_exe failed: {e:#}");
            return;
        }
    };
    let args: Vec<String> = std::env::args().skip(1).collect();
    match std::process::Command::new(&exe).args(&args).spawn() {
        Ok(child) => log::info!("spawned restart (pid {})", child.id()),
        Err(e) => log::error!("restart spawn failed: {e:#}"),
    }
}
