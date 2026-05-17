// Spectrum bar visualizer + peak hold marker.
// バーの上に「直近のピーク位置」を細い線で残す。
//
// Rust 側 (render.rs) は peaks (生ピーク) と peak_ages (経過秒) を毎フレーム
// 流すだけで、hold / decay の挙動はすべてこのシェーダー内で組む:
//   - 最初の HOLD_TIME 秒間はピーク位置で完全に静止 (default: 0、つまり即落下)
//   - そのあと DECAY_PER_SEC の線形 decay で落ちる
//   - 加えて age に応じてアルファをフェードさせる (FADE_TIME を大きくすると無効)
//
// HOLD_TIME / DECAY_PER_SEC / FADE_TIME はこのファイルだけ書き換えて
// ホットリロードすれば即反映される。

const N_BARS: u32 = 96u;

// ピーク位置で静止する時間 (秒)。0 にすると即落下開始 = 純粋な falling 表示。
const HOLD_TIME: f32 = 0.0;
// hold 後の落下速度 (正規化値 / 秒)。0.7 なら 1.0 → 0 に約 1.4 秒。
const DECAY_PER_SEC: f32 = 0.7;
// アルファが 0 になるまでの age (秒)。デフォルトは実質無効 (落下中はずっとフル輝度)。
// 1.5 などの小さい値にすると age に応じて薄れるエフェクトが乗る。
const FADE_TIME: f32 = 1.0e9;

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
@group(0) @binding(3) var<storage, read> peaks: Bars;
@group(0) @binding(4) var<storage, read> peak_ages: Bars;

struct VsOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
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
    _ = wave[0];
    _ = globals.time;

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

    // ピーク位置の決定: hold → linear decay。
    let raw_peak = peaks.data[idx];
    let age = peak_ages.data[idx];
    let decay_elapsed = max(0.0, age - HOLD_TIME);
    let peak_y = max(0.0, raw_peak - DECAY_PER_SEC * decay_elapsed);

    // ピーク線本体。横幅はバーと完全に一致 (bar_mask 共有)。
    let line_thickness: f32 = 0.015;
    let peak_band = 1.0 - smoothstep(0.0, line_thickness, abs(uv.y - peak_y));
    // ピークとバーが重なってる間はフェードアウトして二重発光を抑える
    let separation = smoothstep(0.005, 0.04, peak_y - amp);
    // 経過時間に応じた薄れ。FADE_TIME に近づくほど消える。
    let age_alpha = 1.0 - smoothstep(0.0, FADE_TIME, age);
    let peak_intensity = peak_band * bar_mask * separation * age_alpha;

    let hue = f32(idx) / f32(N_BARS);
    let r = 0.5 + 0.5 * cos(6.2831 * (hue + 0.0));
    let g = 0.5 + 0.5 * cos(6.2831 * (hue + 0.33));
    let b = 0.5 + 0.5 * cos(6.2831 * (hue + 0.66));
    let bar_brightness = 0.55 + 0.45 * amp;
    let col = vec3<f32>(r, g, b) * bar_brightness;
    // ピーク線はバー本体より白寄せして「残光」感を出す
    let peak_col = mix(vec3<f32>(r, g, b), vec3<f32>(1.0, 1.0, 1.0), 0.35);

    let bg = vec3<f32>(0.02, 0.02, 0.04);
    let bar_col = mix(bg, col, in_bar);
    return vec4<f32>(bar_col + col * glow + peak_col * peak_intensity * 0.7, 1.0);
}
