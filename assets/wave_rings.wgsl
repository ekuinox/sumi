// 3 色オシロリング。
// 円周方向に時間領域波形をマッピングし、角度に微小オフセットを足した同じ波形を
// 3 色 (ピンク / シアン / イエロー) で重ね描きする。
//
// 「ぐるぐる回って見える」のは仕様 (時間が進むと wave[] の中身が左にスクロール
// するため、波が円周方向に流れる)。回したくない場合は band_rings.wgsl を使う。

const N_BARS: u32 = 96u;
const PI: f32 = 3.14159265;
const TAU: f32 = 6.28318530;

struct Bars {
    data: array<f32, N_BARS>,
};

struct Globals {
    resolution: vec2<f32>,
    time: f32,
    _pad: f32,
};

@group(0) @binding(0) var<storage, read> bars: Bars;
@group(0) @binding(1) var<uniform> globals: Globals;
@group(0) @binding(2) var<storage, read> wave: array<f32>;

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

// 角度 → 波形値 (cubic smoothstep 補間)
fn sample_wave(angle: f32) -> f32 {
    let len = arrayLength(&wave);
    let t = (angle + PI) / TAU;       // [0, 1)
    let pos = t * f32(len);
    let i0 = u32(floor(pos)) % len;
    let i1 = (i0 + 1u) % len;
    let f = fract(pos);
    let sm = f * f * (3.0 - 2.0 * f);
    return mix(wave[i0], wave[i1], sm);
}

// 角度方向の Gaussian 平均で高周波を落として滑らかにする。
// span を大きくするほどなだらか / 小さくするほど波形の細部が出る。
fn sample_wave_smooth(angle: f32) -> f32 {
    let span: f32 = 0.06; // ≒ 3.4°、bass 主体になる程度
    let taps: i32 = 9;
    var sum = 0.0;
    var weight = 0.0;
    for (var i: i32 = 0; i < taps; i = i + 1) {
        let u = f32(i) / f32(taps - 1) - 0.5;      // [-0.5, 0.5]
        let offset = u * span;
        let w = exp(-pow(u * 2.8, 2.0));            // Gaussian
        sum = sum + sample_wave(angle + offset) * w;
        weight = weight + w;
    }
    return sum / weight;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    _ = bars.data[0];

    let res = globals.resolution;
    let min_side = min(res.x, res.y);
    let p = (in.uv * res - res * 0.5) / (min_side * 0.5);

    let r = length(p);
    let theta = atan2(p.y, p.x);

    // 共通パラメータ
    let base_r = 0.45;
    let displace = 0.08;        // 波形振幅 → 半径方向の歪み
    let wave_gain = 1.8;        // 8x ダウンサンプル後の小振幅補正
    let line_core = 0.0035;     // 線の太さ (画面短辺基準)
    let line_glow = 0.020;      // グロー半径

    // 3 ライン: 色と位相オフセット
    var color_pink   = vec3<f32>(1.00, 0.22, 0.60);
    var color_cyan   = vec3<f32>(0.18, 0.85, 1.00);
    var color_yellow = vec3<f32>(0.98, 0.92, 0.20);

    let phase_pink   =  0.00;
    let phase_cyan   =  0.07;
    let phase_yellow = -0.06;

    var rgb = vec3<f32>(0.0);

    rgb = rgb + draw_line(theta, r, phase_pink,   base_r, displace, wave_gain, line_core, line_glow, color_pink);
    rgb = rgb + draw_line(theta, r, phase_cyan,   base_r, displace, wave_gain, line_core, line_glow, color_cyan);
    rgb = rgb + draw_line(theta, r, phase_yellow, base_r, displace, wave_gain, line_core, line_glow, color_yellow);

    // 暗い背景 + 微かな縦グラデ
    let bg_top = vec3<f32>(0.030, 0.020, 0.045);
    let bg_bot = vec3<f32>(0.050, 0.035, 0.075);
    let bg = mix(bg_bot, bg_top, in.uv.y);

    return vec4<f32>(bg + rgb, 1.0);
}

// 1 本のオシロリングを描画して RGB 寄与を返す。
fn draw_line(
    theta: f32, r: f32, phase: f32,
    base_r: f32, displace: f32, wave_gain: f32,
    core: f32, glow: f32,
    color: vec3<f32>,
) -> vec3<f32> {
    let w = clamp(sample_wave_smooth(theta + phase) * wave_gain, -1.0, 1.0);
    let r_at_theta = base_r + w * displace;
    let d = abs(r - r_at_theta);

    // コア (鋭い線) + グロー (柔らかい広がり)
    let core_intensity = exp(-pow(d / core, 2.0));
    let glow_intensity = exp(-d / glow) * 0.35;

    return color * (core_intensity + glow_intensity);
}
