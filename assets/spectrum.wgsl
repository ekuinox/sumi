// Spectrum bar visualizer.
// `bars` is fed from CPU each frame, length matches dsp::N_BARS.

const N_BARS: u32 = 96u;

struct Bars {
    data: array<f32, N_BARS>,
};

struct Globals {
    resolution: vec2<f32>,
    time: f32,
    orientation: u32,
};

@group(0) @binding(0) var<storage, read> bars: Bars;
@group(0) @binding(1) var<uniform> globals: Globals;
// 未使用。bind group layout の整合のため宣言だけしておく
@group(0) @binding(2) var<storage, read> wave: array<f32>;

struct VsOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    // Fullscreen triangle.
    var pos = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 3.0, -1.0),
        vec2<f32>(-1.0,  3.0),
    );
    var out: VsOut;
    let p = pos[vi];
    out.clip_position = vec4<f32>(p, 0.0, 1.0);
    out.uv = (p + vec2<f32>(1.0)) * 0.5;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // bind group layout の整合のため、未使用バインディングを phony assignment で
    // 参照だけしておく (`_ = 式` は値を捨てる WGSL 専用構文。amp の計算には一切影響しない)
    _ = wave[0];
    _ = globals.time;

    // orientation で UV を回転。0=normal (バーが下から上に伸びる) / 1=90°CW /
    // 2=180° / 3=90°CCW。これにより画面の上下左右どの辺に窓を置いても
    // 同じバー描画ロジックが使える。
    var uv = in.uv;
    if (globals.orientation == 1u) {
        uv = vec2<f32>(uv.y, 1.0 - uv.x);
    } else if (globals.orientation == 2u) {
        uv = vec2<f32>(1.0 - uv.x, 1.0 - uv.y);
    } else if (globals.orientation == 3u) {
        uv = vec2<f32>(1.0 - uv.y, uv.x);
    }
    let bar_f = uv.x * f32(N_BARS);
    let idx = u32(clamp(bar_f, 0.0, f32(N_BARS) - 1.0));
    let amp = bars.data[idx];

    // バー幅の何%を中身として塗るか (残りは隙間)
    let bar_fill = 0.4;
    let local = fract(bar_f);
    let bar_mask = step(abs(local - 0.5), bar_fill * 0.5);

    // uv.y = 0 が画面下、= 1 が上。下から amp の高さまで塗る。
    let in_bar = step(uv.y, amp) * bar_mask;

    // バーの天井 (uv.y ≒ amp) に淡いグロー
    let glow = exp(-pow((uv.y - amp) * 30.0, 2.0)) * 0.5 * bar_mask;

    let hue = f32(idx) / f32(N_BARS);
    let r = 0.5 + 0.5 * cos(6.2831 * (hue + 0.0));
    let g = 0.5 + 0.5 * cos(6.2831 * (hue + 0.33));
    let b = 0.5 + 0.5 * cos(6.2831 * (hue + 0.66));
    let col = vec3<f32>(r, g, b) * (0.55 + 0.45 * amp);

    let bg = vec3<f32>(0.02, 0.02, 0.04);
    let bar_col = mix(bg, col, in_bar);
    return vec4<f32>(bar_col + col * glow, 1.0);
}
