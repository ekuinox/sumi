// 3 本リング (帯域別 + スクロールなし) ビジュアライザ。wave_rings.wgsl の固定版。
// 3 本は別々の周波数帯 (bass / mid / treble) を担当し、それぞれ
// 「帯域全体の音量で円全体が膨らむ」+「角度方向の小さな凹凸」で動く。
// バランス的にほぼ真円が呼吸する + 細かい揺れが乗る。

const N_BARS: u32 = 96u;
const PI: f32 = 3.14159265;
const TAU: f32 = 6.28318530;

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

// 指定範囲の bars を平均 (帯域の全体エネルギー)
fn band_energy(start: u32, end: u32) -> f32 {
    var sum = 0.0;
    var count = 0.0;
    for (var i: u32 = start; i < end; i = i + 1u) {
        sum = sum + bars.data[i];
        count = count + 1.0;
    }
    return sum / count;
}

// 角度 → 指定範囲内の bar 値 (左右対称マッピング)。
// この帯域だけ見て、角度ごとの細かい凹凸を取る用。
fn band_local(angle: f32, start: u32, end: u32) -> f32 {
    let a_top = atan2(abs(sin(angle)), cos(angle));   // [0, PI]
    let t = clamp(a_top / PI, 0.0, 1.0);
    let band_n = f32(end - start);
    let pos = t * (band_n - 1.0);
    let i0 = u32(floor(pos)) + start;
    let i1 = min(i0 + 1u, end - 1u);
    let f = fract(pos);
    let sm = f * f * (3.0 - 2.0 * f);
    return mix(bars.data[i0], bars.data[i1], sm);
}

// 角度方向に Gaussian で平滑
fn band_local_smooth(angle: f32, start: u32, end: u32) -> f32 {
    let span: f32 = 0.50;
    let taps: i32 = 17;
    var sum = 0.0;
    var weight = 0.0;
    for (var i: i32 = 0; i < taps; i = i + 1) {
        let u = f32(i) / f32(taps - 1) - 0.5;
        let offset = u * span;
        let w = exp(-pow(u * 2.8, 2.0));
        sum = sum + band_local(angle + offset, start, end) * w;
        weight = weight + w;
    }
    return sum / weight;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    _ = wave[0];

    let res = globals.resolution;
    let min_side = min(res.x, res.y);
    let p = (in.uv * res - res * 0.5) / (min_side * 0.5);

    let r = length(p);
    let theta = atan2(p.y, p.x);

    let line_core = 0.0035;
    let line_glow = 0.020;

    // 3 本 — それぞれ別の帯域、別の半径
    let color_pink   = vec3<f32>(1.00, 0.22, 0.60);
    let color_cyan   = vec3<f32>(0.18, 0.85, 1.00);
    let color_yellow = vec3<f32>(0.98, 0.92, 0.20);

    // bars インデックス: 0=最低音, N-1=最高音
    // bass(下 1/3), mid(中央 1/3), treble(上 1/3)
    // 3 本とも同じ半径 = 同じ円周上で重なり合い、音で分離する
    let ring_r = 0.46;
    var rgb = vec3<f32>(0.0);
    rgb = rgb + draw_ring(theta, r, 0u,  32u, ring_r, color_pink,   line_core, line_glow);
    rgb = rgb + draw_ring(theta, r, 32u, 64u, ring_r, color_cyan,   line_core, line_glow);
    rgb = rgb + draw_ring(theta, r, 64u, 96u, ring_r, color_yellow, line_core, line_glow);

    let bg_top = vec3<f32>(0.030, 0.020, 0.045);
    let bg_bot = vec3<f32>(0.050, 0.035, 0.075);
    let bg = mix(bg_bot, bg_top, in.uv.y);

    return vec4<f32>(bg + rgb, 1.0);
}

// 帯域全体の音量で半径が均一に膨らむ + 角度方向に小さな凹凸を乗せる
fn draw_ring(
    theta: f32, r: f32,
    bar_start: u32, bar_end: u32,
    base_r: f32,
    color: vec3<f32>,
    core: f32, glow: f32,
) -> vec3<f32> {
    let breathe = band_energy(bar_start, bar_end);          // 全体で膨らむ量
    let wobble  = band_local_smooth(theta, bar_start, bar_end); // 角度ごとの凹凸

    // 比率: 0.8 = 帯域全体エネルギー、0.2 = 角度凹凸。これで「ほぼ真円が呼吸 + 微小な揺らぎ」
    let amp = breathe * 0.8 + wobble * 0.4;
    let push = 0.10;
    let r_at_theta = base_r + clamp(amp * 2.0, 0.0, 1.5) * push;

    let d = abs(r - r_at_theta);
    let core_i = exp(-pow(d / core, 2.0));
    let glow_i = exp(-d / glow) * 0.35;
    return color * (core_i + glow_i);
}
