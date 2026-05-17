// 黄色のリング + 内側ドットフィールド型ビジュアライザ。
// - 外枠: 黄色の太いリング、スペクトラムでうっすら歪む
// - 内側: ドットの集合が音に合わせて流れる (3D 波の表面っぽい印象)
// - 背景: 縦グラデの濃紺 + ランダム星

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
// 未使用。bind group layout の整合のため宣言だけしておく
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

// 真上 (+Y) からの角度に対する bar 値 (左右対称マッピング)
fn sample_bar_at(px: f32, py: f32) -> f32 {
    let a = atan2(abs(px), py);
    let t = clamp(a / PI, 0.0, 1.0);
    let pos = t * f32(N_BARS - 1u);
    let i0 = u32(floor(pos));
    let i1 = min(i0 + 1u, N_BARS - 1u);
    let f = fract(pos);
    let sm = f * f * (3.0 - 2.0 * f);
    return mix(bars.data[i0], bars.data[i1], sm);
}

fn hash2(p: vec2<f32>) -> vec2<f32> {
    let h1 = fract(sin(dot(p, vec2<f32>(127.1, 311.7))) * 43758.5453);
    let h2 = fract(sin(dot(p, vec2<f32>(269.5, 183.3))) * 43758.5453);
    return vec2<f32>(h1, h2);
}

// 全 bar の平均 (= 全体の音圧概略)
fn overall_energy() -> f32 {
    var sum = 0.0;
    for (var i: u32 = 0u; i < N_BARS; i = i + 1u) {
        sum = sum + bars.data[i];
    }
    return sum / f32(N_BARS);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    _ = wave[0];
    _ = peaks.data[0];
    _ = peak_ages.data[0];

    let res = globals.resolution;
    let min_side = min(res.x, res.y);
    let p = (in.uv * res - res * 0.5) / (min_side * 0.5);

    let r = length(p);

    // --- リング (黄色アウトライン) ---
    let base_r = 0.55;
    let max_push = 0.06;
    let bar_here = sample_bar_at(p.x, p.y);
    let r_at_angle = base_r + bar_here * max_push;

    let ring_thickness = 0.025;
    let dist = abs(r - r_at_angle);
    let ring_core = exp(-pow(dist / ring_thickness, 2.0));
    let outer_glow = exp(-max(r - r_at_angle, 0.0) * 11.0) * 0.45;
    let inner_glow = exp(-max(r_at_angle - r, 0.0) * 13.0) * 0.30;

    // --- 内側の粒子フィールド ---
    // リング内側端からフェードインさせる
    let inside = smoothstep(r_at_angle - ring_thickness * 0.5,
                            r_at_angle - ring_thickness * 6.0, r);

    let energy = overall_energy();
    let local_e = bar_here;

    // ドットグリッド (波で歪める)
    let grid = 36.0;
    let cell_uv = p * grid;
    let cell_id = floor(cell_uv);
    let cell_local = fract(cell_uv) - 0.5;

    // セルごとのランダムオフセット (基本的な不揃いさ)
    let h = hash2(cell_id);
    let base_off = (h - 0.5) * 0.5;

    // 音と時間で歪み: 3D 波の表面っぽい流れ
    let wave_amp = energy * 1.2;
    let warp = vec2<f32>(
        sin(cell_id.y * 0.4 + globals.time * 0.9 + cell_id.x * 0.15) * wave_amp,
        cos(cell_id.x * 0.35 + globals.time * 0.7) * wave_amp * 0.6,
    );

    let particle_pos = cell_local + base_off + warp;
    let dot_d = length(particle_pos);

    // ドットの明るさ: 中央寄りで強く、エネルギーで強調、内側マスクで切り取り
    let dot_core = 1.0 - smoothstep(0.04, 0.10, dot_d);
    let particle_amount = dot_core * (0.25 + (energy + local_e * 0.5) * 2.0) * inside;

    // 各セルにランダム ON/OFF (`h.x` が小さいセルは消しておく) → 寂しい時に密度減
    let cell_visible = step(h.x, 0.35 + energy * 0.6);
    let particles = particle_amount * cell_visible;

    // --- カラー ---
    let yellow = vec3<f32>(0.97, 0.92, 0.18);
    let highlight = vec3<f32>(1.00, 1.00, 0.75);
    let ring_col = mix(yellow, highlight, ring_core);

    // --- 背景: 縦グラデ + 細かい星 ---
    let bg_top = vec3<f32>(0.020, 0.025, 0.055);
    let bg_bot = vec3<f32>(0.045, 0.055, 0.10);
    let bg = mix(bg_bot, bg_top, in.uv.y);
    let star_n = fract(sin(dot(in.uv * res, vec2<f32>(12.9898, 78.233))) * 43758.5453);
    let stars = step(0.996, star_n) * 0.6;

    var rgb = bg + vec3<f32>(stars);
    rgb = rgb + yellow * particles;
    rgb = rgb + ring_col * ring_core * 1.5;
    rgb = rgb + yellow * (outer_glow + inner_glow);

    return vec4<f32>(rgb, 1.0);
}
