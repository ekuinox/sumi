//! 任意の WASAPI 入力デバイスから 1 回録音して wav に保存する最小サンプル。
//!
//! Process Loopback Capture は WASAPI 排他モードを取り込めない仕様だが、
//! オーディオインターフェース側に出力を入力にミックスして返すループバック機能が
//! あれば、それは Windows から見ると単なる入力デバイスなので普通に capture できる。
//! その経路の生存確認用。

use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;
use std::ptr;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context as _, Result};
use clap::Parser;
use duration_str::parse_std;
use wav::{BitDepth, Header};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Media::Audio::{
    eCapture, IAudioCaptureClient, IAudioClient, IMMDevice, IMMDeviceEnumerator,
    MMDeviceEnumerator, AUDCLNT_SHAREMODE_SHARED, DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ,
};

const WAVE_FORMAT_IEEE_FLOAT: u16 = 0x0003;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

#[derive(Parser, Debug)]
struct Cli {
    /// 出力 wav パス (省略時は録音せず、デバイス列挙のみ)
    #[clap(short, long)]
    output: Option<PathBuf>,
    /// 録音時間
    #[clap(short, long, default_value = "10s")]
    duration: String,
    /// 対象入力デバイスの friendly name (部分一致, 大文字小文字無視)
    #[clap(long, default_value = "Mix 3/4")]
    device: String,
}

fn main() -> Result<()> {
    std::env::set_var("RUST_LOG", "INFO");
    env_logger::init();

    let cli = Cli::parse();
    let duration = parse_std(&cli.duration).map_err(|e| anyhow!("duration parse: {e}"))?;

    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok()? };

    let devices = unsafe { list_capture_devices()? };
    log::info!("--- capture devices ---");
    for (i, (name, _)) in devices.iter().enumerate() {
        log::info!("  [{i}] {name}");
    }
    log::info!("--- end capture devices ---");

    let needle = cli.device.to_ascii_lowercase();
    let picked = devices
        .iter()
        .find(|(name, _)| name.to_ascii_lowercase().contains(&needle))
        .with_context(|| format!("device matching {:?} not found", cli.device))?;
    log::info!("picked: {}", picked.0);

    let (buffer, fmt) = unsafe { capture(&picked.1, duration)? };
    log::info!(
        "captured {} bytes, sample_rate={}Hz channels={} bits={} tag=0x{:04x}",
        buffer.len(),
        fmt.samples_per_sec,
        fmt.channels,
        fmt.bits_per_sample,
        fmt.format_tag
    );

    if let Some(out) = &cli.output {
        write_wav(out, &buffer, &fmt)?;
        log::info!("wrote {}", out.display());
    }

    unsafe { CoUninitialize() };
    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct FormatInfo {
    format_tag: u16,
    channels: u16,
    samples_per_sec: u32,
    bits_per_sample: u16,
    block_align: u16,
}

unsafe fn list_capture_devices() -> Result<Vec<(String, IMMDevice)>> {
    let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
    let collection = enumerator.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE)?;
    let count = collection.GetCount()?;
    let mut out = Vec::with_capacity(count as usize);
    for i in 0..count {
        let dev = collection.Item(i)?;
        let store = dev.OpenPropertyStore(STGM_READ)?;
        let value = store.GetValue(&PKEY_Device_FriendlyName)?;
        let name = value.to_string();
        out.push((name, dev));
    }
    Ok(out)
}

unsafe fn capture(device: &IMMDevice, duration: Duration) -> Result<(Vec<u8>, FormatInfo)> {
    let client: IAudioClient = device
        .Activate(CLSCTX_ALL, None)
        .context("IMMDevice::Activate(IAudioClient) failed")?;

    // デバイスのネイティブ mix format をそのまま使う (float か PCM かはドライバ依存)。
    let mix_format_ptr = client.GetMixFormat().context("GetMixFormat failed")?;
    let mix_format = *mix_format_ptr;
    let fmt = FormatInfo {
        format_tag: mix_format.wFormatTag,
        channels: mix_format.nChannels,
        samples_per_sec: mix_format.nSamplesPerSec,
        bits_per_sample: mix_format.wBitsPerSample,
        block_align: mix_format.nBlockAlign,
    };
    log::info!(
        "mix_format: tag=0x{:04x} ch={} rate={} bits={} block_align={}",
        fmt.format_tag,
        fmt.channels,
        fmt.samples_per_sec,
        fmt.bits_per_sample,
        fmt.block_align
    );

    // 10 秒分のバッファ
    let buffered_duration = Duration::from_secs(10);
    client
        .Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            0,
            buffered_duration.as_micros() as i64 * 10,
            0,
            mix_format_ptr,
            None,
        )
        .context("IAudioClient::Initialize failed")?;

    let capture_client: IAudioCaptureClient = client.GetService()?;
    client.Start()?;

    let mut buffer_all: Vec<u8> = Vec::with_capacity(
        fmt.samples_per_sec as usize * fmt.block_align as usize * 10,
    );
    let mut peak: f32 = 0.0;
    let mut packets: u64 = 0;
    let mut silent_packets: u64 = 0;
    const AUDCLNT_BUFFERFLAGS_SILENT: u32 = 0x2;
    let started_at = Instant::now();

    while started_at.elapsed() < duration {
        let mut buffer_ptr: *mut u8 = ptr::null_mut();
        let mut stored_frames: u32 = 0;
        let mut flags: u32 = 0;
        capture_client.GetBuffer(
            &mut buffer_ptr,
            &mut stored_frames,
            &mut flags,
            None,
            None,
        )?;
        if stored_frames == 0 {
            std::thread::sleep(Duration::from_millis(2));
            continue;
        }
        packets += 1;
        let len = (stored_frames * fmt.block_align as u32) as usize;
        if (flags & AUDCLNT_BUFFERFLAGS_SILENT) != 0 || buffer_ptr.is_null() {
            silent_packets += 1;
            buffer_all.extend(std::iter::repeat(0u8).take(len));
        } else {
            let slice = std::slice::from_raw_parts(buffer_ptr, len);
            peak = peak.max(estimate_peak(slice, &fmt));
            buffer_all.extend_from_slice(slice);
        }
        capture_client.ReleaseBuffer(stored_frames)?;
    }
    client.Stop()?;
    log::info!("packets={packets} silent={silent_packets} peak={peak:.6}");
    Ok((buffer_all, fmt))
}

fn estimate_peak(bytes: &[u8], fmt: &FormatInfo) -> f32 {
    let bytes_per_sample = (fmt.bits_per_sample / 8) as usize;
    let is_float = fmt.format_tag == WAVE_FORMAT_IEEE_FLOAT
        || (fmt.format_tag == WAVE_FORMAT_EXTENSIBLE && fmt.bits_per_sample == 32);
    let mut peak = 0.0_f32;
    for chunk in bytes.chunks_exact(bytes_per_sample) {
        let v = if is_float && bytes_per_sample == 4 {
            f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]).abs()
        } else if bytes_per_sample == 2 {
            (i16::from_le_bytes([chunk[0], chunk[1]]).abs() as f32) / 32768.0
        } else if bytes_per_sample == 4 {
            // 32-bit PCM
            (i32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]).abs() as f32)
                / 2_147_483_648.0
        } else {
            0.0
        };
        if v > peak {
            peak = v;
        }
    }
    peak
}

fn write_wav(path: &std::path::Path, bytes: &[u8], fmt: &FormatInfo) -> Result<()> {
    let (wav_fmt_tag, payload) = if fmt.format_tag == WAVE_FORMAT_IEEE_FLOAT
        || (fmt.format_tag == WAVE_FORMAT_EXTENSIBLE && fmt.bits_per_sample == 32)
    {
        let samples: Vec<f32> = bytes
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        (wav::WAV_FORMAT_IEEE_FLOAT, BitDepth::ThirtyTwoFloat(samples))
    } else if fmt.bits_per_sample == 16 {
        let samples: Vec<i16> = bytes
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();
        (wav::WAV_FORMAT_PCM, BitDepth::Sixteen(samples))
    } else {
        bail!(
            "unsupported format for wav writer: tag=0x{:04x} bits={}",
            fmt.format_tag,
            fmt.bits_per_sample
        );
    };
    let _ = wav_fmt_tag; // header builder handles tag internally
    let header = Header::new(
        if matches!(payload, BitDepth::ThirtyTwoFloat(_)) {
            wav::WAV_FORMAT_IEEE_FLOAT
        } else {
            wav::WAV_FORMAT_PCM
        },
        fmt.channels,
        fmt.samples_per_sec,
        fmt.bits_per_sample,
    );
    let mut writer = BufWriter::new(File::create(path)?);
    wav::write(header, &payload, &mut writer)?;
    Ok(())
}

