//! wgpu setup + spectrum-bar render pipeline.
//!
//! Shader source is supplied at construction and can be hot-replaced via
//! `reload_shader`. Validation errors are caught by an error scope so a broken
//! WGSL keeps the previous pipeline alive instead of crashing.

use std::sync::Arc;
use std::time::Instant;

use anyhow::{anyhow, Context as _, Result};
use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;
use winit::dpi::PhysicalSize;
use winit::window::Window;

use crate::dsp::N_BARS;

/// Number of mono samples uploaded each frame for waveform rendering. The CPU
/// side downsamples the raw audio ring into exactly this many points.
pub const WAVE_LEN: usize = 256;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct BarsBlock {
    data: [f32; N_BARS],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Globals {
    resolution: [f32; 2],
    time: f32,
    /// バー描画系シェーダーが UV を回転させるためのヒント
    /// 0=normal (bars grow up), 1=90° CW, 2=180°, 3=90° CCW
    orientation: u32,
}

pub struct Renderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline_layout: wgpu::PipelineLayout,
    pipeline: wgpu::RenderPipeline,
    bars_buffer: wgpu::Buffer,
    wave_buffer: wgpu::Buffer,
    globals_buffer: wgpu::Buffer,
    peaks_buffer: wgpu::Buffer,
    peak_ages_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    start_time: Instant,
    orientation: u32,
    /// バーごとの生ピーク値 (binding(3))。`bars[i] >= peaks[i]` のときだけ更新される。
    /// Rust 側では一切減衰させない。hold / decay の挙動はシェーダー側で
    /// `peak_ages_sec[i]` を見て自由に組める。
    peaks: Box<[f32; N_BARS]>,
    /// 各 bar のピークが最後に更新されてからの経過秒 (binding(4))。
    /// `bars[i] >= peaks[i]` で 0 にリセット、それ以外は dt を加算。
    peak_ages_sec: Box<[f32; N_BARS]>,
    last_peak_update: Instant,
    pub size: PhysicalSize<u32>,
}

impl Renderer {
    pub async fn new(window: Arc<Window>, shader_source: &str) -> Result<Self> {
        let size = window.inner_size();
        let size = PhysicalSize::new(size.width.max(1), size.height.max(1));

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            ..Default::default()
        });

        let surface = instance
            .create_surface(window.clone())
            .context("create_surface")?;

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .ok_or_else(|| anyhow!("no compatible adapter"))?;

        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("sumi-device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::downlevel_defaults(),
                    memory_hints: wgpu::MemoryHints::Performance,
                },
                None,
            )
            .await
            .context("request_device")?;

        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(caps.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width,
            height: size.height,
            present_mode: wgpu::PresentMode::AutoVsync,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let bars_zero = BarsBlock {
            data: [0.0; N_BARS],
        };
        let bars_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("bars"),
            contents: bytemuck::bytes_of(&bars_zero),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        let globals_initial = Globals {
            resolution: [size.width as f32, size.height as f32],
            time: 0.0,
            orientation: 0,
        };
        let globals_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("globals"),
            contents: bytemuck::bytes_of(&globals_initial),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let wave_zero = vec![0.0_f32; WAVE_LEN];
        let wave_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("wave"),
            contents: bytemuck::cast_slice(&wave_zero),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        let peaks_zero = BarsBlock {
            data: [0.0; N_BARS],
        };
        let peaks_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("peaks"),
            contents: bytemuck::bytes_of(&peaks_zero),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });
        let peak_ages_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("peak_ages"),
            contents: bytemuck::bytes_of(&peaks_zero),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bg-layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bg"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: bars_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: globals_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wave_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: peaks_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: peak_ages_buffer.as_entire_binding(),
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pipeline-layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        // Initial pipeline must succeed; if it fails, propagate so startup errors out
        // (we can't run with no pipeline).
        let pipeline = build_pipeline(&device, &pipeline_layout, config.format, shader_source)
            .map_err(|e| anyhow!("initial shader failed to compile:\n{e}"))?;

        let now = Instant::now();
        Ok(Self {
            surface,
            device,
            queue,
            config,
            pipeline_layout,
            pipeline,
            bars_buffer,
            wave_buffer,
            globals_buffer,
            peaks_buffer,
            peak_ages_buffer,
            bind_group,
            start_time: now,
            orientation: 0,
            peaks: Box::new([0.0; N_BARS]),
            peak_ages_sec: Box::new([0.0; N_BARS]),
            last_peak_update: now,
            size,
        })
    }

    /// バー描画系シェーダー向けの回転ヒントを設定する。
    /// 値は 0=normal / 1=90°CW / 2=180° / 3=90°CCW。
    pub fn set_orientation(&mut self, orientation: u32) {
        self.orientation = orientation;
    }

    pub fn resize(&mut self, new_size: PhysicalSize<u32>) {
        if new_size.width == 0 || new_size.height == 0 {
            return;
        }
        self.size = new_size;
        self.config.width = new_size.width;
        self.config.height = new_size.height;
        self.surface.configure(&self.device, &self.config);
    }

    pub fn update(&mut self, bars: &[f32; N_BARS], wave: &[f32; WAVE_LEN]) {
        // ピーク更新: bars が現ピーク以上なら refresh (値を更新、age を 0 にリセット)、
        // それ以外なら age に dt を加算するだけ。減衰や hold の解釈はシェーダーに任せる。
        // dt はウィンドウが背景行きで止まったあとの巨大値を避けて 100ms に頭打ち。
        let now = Instant::now();
        let dt = now
            .duration_since(self.last_peak_update)
            .as_secs_f32()
            .min(0.1);
        self.last_peak_update = now;
        for (i, &b) in bars.iter().enumerate() {
            if b >= self.peaks[i] {
                self.peaks[i] = b;
                self.peak_ages_sec[i] = 0.0;
            } else {
                self.peak_ages_sec[i] += dt;
            }
        }

        self.queue
            .write_buffer(&self.bars_buffer, 0, bytemuck::cast_slice(bars));
        self.queue
            .write_buffer(&self.wave_buffer, 0, bytemuck::cast_slice(wave));
        self.queue.write_buffer(
            &self.peaks_buffer,
            0,
            bytemuck::cast_slice(self.peaks.as_ref()),
        );
        self.queue.write_buffer(
            &self.peak_ages_buffer,
            0,
            bytemuck::cast_slice(self.peak_ages_sec.as_ref()),
        );
        let globals = Globals {
            resolution: [self.size.width as f32, self.size.height as f32],
            time: self.start_time.elapsed().as_secs_f32(),
            orientation: self.orientation,
        };
        self.queue
            .write_buffer(&self.globals_buffer, 0, bytemuck::bytes_of(&globals));
    }

    /// Replace the active pipeline with one compiled from `source`. On any
    /// validation error the old pipeline is kept and the error is returned.
    pub fn reload_shader(&mut self, source: &str) -> Result<(), String> {
        match build_pipeline(
            &self.device,
            &self.pipeline_layout,
            self.config.format,
            source,
        ) {
            Ok(p) => {
                self.pipeline = p;
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    pub fn render(&self) -> Result<(), wgpu::SurfaceError> {
        let frame = self.surface.get_current_texture()?;
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("encoder"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit(std::iter::once(encoder.finish()));
        frame.present();
        Ok(())
    }
}

/// Try to compile a WGSL source into a render pipeline. Validation errors are
/// captured via an error scope and returned as a String instead of panicking.
fn build_pipeline(
    device: &wgpu::Device,
    pipeline_layout: &wgpu::PipelineLayout,
    color_format: wgpu::TextureFormat,
    source: &str,
) -> std::result::Result<wgpu::RenderPipeline, String> {
    device.push_error_scope(wgpu::ErrorFilter::Validation);

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("spectrum.wgsl"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });

    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("spectrum-pipeline"),
        layout: Some(pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: "vs_main",
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: "fs_main",
            targets: &[Some(wgpu::ColorTargetState {
                format: color_format,
                blend: Some(wgpu::BlendState::REPLACE),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    });

    match pollster::block_on(device.pop_error_scope()) {
        Some(err) => Err(format!("{err}")),
        None => Ok(pipeline),
    }
}
