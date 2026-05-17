mod audio;
mod dsp;
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
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowAttributes, WindowId};

use crate::audio::{spawn_capture, CaptureHandle};
use crate::dsp::{Dsp, FFT_SIZE, N_BARS};
use crate::render::{Renderer, WAVE_LEN};
use crate::tray::Tray;

#[derive(Parser, Debug)]
struct Cli {
    /// 入力デバイスの friendly name 部分一致。`mmsys.cpl` 録音タブで見えるデバイス名で指定する。
    /// 排他モード再生中のアプリを拾うには、オーディオ I/F のハードウェアループバック
    /// チャンネル名 (例: `Mix 3/4` のような) を指定する。
    #[clap(long, default_value = "Mix 3/4")]
    device: String,
    /// 使用する WGSL シェーダーファイル。保存するとホットリロードされる。
    #[clap(long, default_value = "assets/spectrum.wgsl")]
    shader: PathBuf,
}

fn main() -> Result<()> {
    if std::env::var_os("RUST_LOG").is_none() {
        std::env::set_var("RUST_LOG", "info,wgpu_core=warn,wgpu_hal=warn,naga=warn");
    }
    env_logger::init();
    let cli = Cli::parse();

    let shader_path = cli.shader.canonicalize().with_context(|| {
        format!("shader file not found: {}", cli.shader.display())
    })?;
    let initial_shader = std::fs::read_to_string(&shader_path)
        .with_context(|| format!("read shader {}", shader_path.display()))?;

    let capture = spawn_capture(cli.device, 1 << 16)?;
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
}

impl App {
    fn restore_window(&self) {
        if let Some(w) = &self.window {
            w.set_minimized(false);
            w.set_visible(true);
            let _ = w.request_inner_size(LogicalSize::new(960.0, 540.0));
            w.focus_window();
        }
    }

    fn drain_tray_events(&self, event_loop: &ActiveEventLoop) {
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
        // メニュー (Show / Quit) のクリック
        while let Ok(ev) = tray_icon::menu::MenuEvent::receiver().try_recv() {
            if ev.id == tray.show_id {
                self.restore_window();
            } else if ev.id == tray.quit_id {
                event_loop.exit();
            }
        }
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
        let attrs = WindowAttributes::default()
            .with_title("chryth")
            .with_inner_size(LogicalSize::new(960.0, 360.0));
        let window = event_loop.create_window(attrs).expect("create window");
        let window = Arc::new(window);
        let renderer = pollster::block_on(Renderer::new(window.clone(), &self.initial_shader))
            .expect("renderer");
        let dsp = Dsp::new(self.capture.format.sample_rate);

        // Tray icon は同じスレッド (= winit event loop = main thread) で作る必要がある。
        match Tray::new("chryth") {
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
