//! Hann window + FFT, with a fixed mapping from FFT bins down to `N_BARS`
//! log-spaced bands so the shader can index a flat array.

use spectrum_analyzer::scaling::divide_by_N;
use spectrum_analyzer::windows::hann_window;
use spectrum_analyzer::{samples_fft_to_spectrum, FrequencyLimit};

/// FFT window size. Must be a power of two.
pub const FFT_SIZE: usize = 4096;

/// Number of bars (log-spaced bands) the shader expects.
pub const N_BARS: usize = 96;

/// Hz range we visualize.
const MIN_HZ: f32 = 30.0;
const MAX_HZ: f32 = 16_000.0;

/// State for the DSP stage: keeps the last spectrum so we can apply a simple
/// per-band peak-decay smoothing across frames.
pub struct Dsp {
    sample_rate: u32,
    bars: [f32; N_BARS],
    band_edges: Vec<(f32, f32)>,
}

impl Dsp {
    pub fn new(sample_rate: u32) -> Self {
        let band_edges = log_band_edges(MIN_HZ, MAX_HZ, N_BARS);
        Self {
            sample_rate,
            bars: [0.0; N_BARS],
            band_edges,
        }
    }

    /// Compute spectrum bars from the latest `FFT_SIZE` samples.
    /// Returns the up-to-date bar array (with per-band peak decay applied).
    pub fn process(&mut self, samples: &[f32; FFT_SIZE]) -> &[f32; N_BARS] {
        let windowed = hann_window(samples);
        let spectrum = match samples_fft_to_spectrum(
            &windowed,
            self.sample_rate,
            FrequencyLimit::Range(MIN_HZ, MAX_HZ),
            Some(&divide_by_N),
        ) {
            Ok(s) => s,
            Err(_) => return &self.bars,
        };

        // dB スケール: divide_by_N 後のマグニチュードは概ね 0〜1。
        // 音楽の実用レンジに合わせて -55dB を底、0dB (= 1.0) を天井に。
        const DB_FLOOR: f32 = -55.0;
        const DB_CEIL: f32 = 0.0;
        const GAMMA: f32 = 0.6;
        const DECAY: f32 = 0.70;

        for (i, (lo, hi)) in self.band_edges.iter().enumerate() {
            let mut peak = 0.0_f32;
            for (freq, mag) in spectrum.data().iter() {
                let f = freq.val();
                if f >= *lo && f < *hi {
                    let v = mag.val();
                    if v > peak {
                        peak = v;
                    }
                }
            }
            let db = 20.0 * peak.max(1e-7).log10();
            let norm = ((db - DB_FLOOR) / (DB_CEIL - DB_FLOOR)).clamp(0.0, 1.0);
            let v = norm.powf(GAMMA);
            let prev = self.bars[i] * DECAY;
            self.bars[i] = v.max(prev);
        }
        &self.bars
    }
}

fn log_band_edges(lo: f32, hi: f32, n: usize) -> Vec<(f32, f32)> {
    let log_lo = lo.ln();
    let log_hi = hi.ln();
    let step = (log_hi - log_lo) / n as f32;
    (0..n)
        .map(|i| {
            let a = (log_lo + step * i as f32).exp();
            let b = (log_lo + step * (i + 1) as f32).exp();
            (a, b)
        })
        .collect()
}
