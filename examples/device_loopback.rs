//! デスクトップ音源を指定した期間で録音してファイルに保存するだけのサンプル

use std::{
    fs::File,
    io::BufWriter,
    mem::ManuallyDrop,
    path::PathBuf,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use clap::Parser;
use duration_str::parse_std;
use wav::{BitDepth, Header};
use windows::Win32::{
    Devices::FunctionDiscovery::PKEY_Device_FriendlyName,
    Media::Audio::{
        eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDevice, IMMDeviceEnumerator,
        MMDeviceEnumerator, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, WAVEFORMATEX,
    },
    System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED,
        STGM_READ,
    },
};

#[derive(Parser, Debug)]
pub struct Cli {
    /// 出力先
    #[clap(short, long)]
    output: PathBuf,

    /// 記録する期間
    #[clap(short, long, default_value = "1m")]
    duration: String,
}

fn main() {
    let cli = Cli::parse();

    std::env::set_var("RUST_LOG", "INFO");
    env_logger::init();

    // clap の ValueParser 通したいけど今は面倒なのでいい
    let duration = parse_std(&cli.duration).expect("Failed to parse duration text.");

    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED)
            .ok()
            .expect("Failed to initialize COM.")
    }

    let (buffer, wave_format) = unsafe {
        let device = get_device().expect("Failed to get IMMDevice.");
        let name = get_device_name(&device).unwrap_or_default();
        log::info!("Device: {name}");
        capture_audio(&device, duration).expect("Failed to capture audio.")
    };

    let mut output =
        BufWriter::new(File::create(&cli.output).expect("Failed to create output file."));

    log::info!("Format: {wave_format:#?}");

    let WaveFormatEx {
        channels,
        samples_per_sec,
        bits_per_sample,
        ..
    } = wave_format;

    // WaveFormatEx::wave_format を無視しているけど、拡張可能オーディオ形式だったとしても保存するときには関係なさそう
    let header = Header::new(
        wav::WAV_FORMAT_IEEE_FLOAT,
        channels,
        samples_per_sec,
        bits_per_sample,
    );
    let buffer = BitDepth::Eight(buffer);

    wav::write(header, &buffer, &mut output).expect("Failed to write buffer.");

    unsafe { CoUninitialize() }
}

unsafe fn get_device() -> Result<IMMDevice> {
    let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
        .context("Failed to create device enumerator.")?;

    let device = enumerator
        .GetDefaultAudioEndpoint(eRender, eConsole)
        .context("Failed to get default audio endpoint.")?;

    Ok(device)
}

unsafe fn capture_audio(device: &IMMDevice, duration: Duration) -> Result<(Vec<u8>, WaveFormatEx)> {
    let audio_client: IAudioClient = device
        .Activate(CLSCTX_ALL, None)
        .context("Failed to activate audio client.")?;

    let wave_format = audio_client
        .GetMixFormat()
        .context("Failed to get mix format.")?;

    let buffered_duration = Duration::from_secs(10);

    audio_client
        .Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK,
            buffered_duration.as_micros() as i64,
            0,
            wave_format,
            None,
        )
        .context("Failed to initialize audio client.")?;
    let wave_format: WaveFormatEx = (*wave_format).into();

    // ChatGPT が言うにはこっちのやり方の方が推奨されるとのことだったが、こっちは実行時エラーになった
    // let capture_client: IAudioCaptureClient = device
    //     .Activate(CLSCTX_ALL, None)
    //     .context("Failed to activate capture client.")?;

    let capture_client: IAudioCaptureClient = audio_client
        .GetService()
        .context("Failed to get capture client.")?;

    audio_client
        .Start()
        .context("Failed to start audio client.")?;

    let mut buffer_all = Vec::<u8>::with_capacity(wave_format.avg_bytes_per_sec as usize * 10);
    let started_at = Instant::now();

    while started_at.elapsed() < duration {
        let frames = audio_client
            .GetCurrentPadding()
            .context("Failed to get current padding.")?;
        if frames == 0 {
            continue;
        }

        let mut buffer_ptr: *mut u8 = &mut 0;
        let mut stored_frames = 0;
        let mut flags = 0;
        capture_client.GetBuffer(&mut buffer_ptr, &mut stored_frames, &mut flags, None, None)?;

        let buffer_length = stored_frames * (wave_format.block_align as u32);

        // drop が走っちゃって死ぬので
        let buffer = ManuallyDrop::new(Vec::from_raw_parts(
            buffer_ptr,
            buffer_length as usize,
            buffer_length as usize,
        ));

        buffer_all.extend(buffer.iter());

        capture_client
            .ReleaseBuffer(stored_frames)
            .context("Failed to release buffer.")?;

        std::thread::sleep(Duration::from_micros(100));
    }

    audio_client.Stop().context("Failed to stop client.")?;

    Ok((buffer_all, wave_format))
}

unsafe fn get_device_name(device: &IMMDevice) -> Result<String> {
    let store = device.OpenPropertyStore(STGM_READ)?;
    let value = store.GetValue(&PKEY_Device_FriendlyName)?;
    Ok(value.to_string())
}

#[derive(Debug)]
pub struct WaveFormatEx {
    pub format_tag: u16,
    pub channels: u16,
    pub samples_per_sec: u32,
    pub avg_bytes_per_sec: u32,
    pub block_align: u16,
    pub bits_per_sample: u16,
    pub size: u16,
}

impl From<WAVEFORMATEX> for WaveFormatEx {
    fn from(value: WAVEFORMATEX) -> Self {
        let format_tag = value.wFormatTag;
        let channels = value.nChannels;
        let samples_per_sec = value.nSamplesPerSec;
        let avg_bytes_per_sec = value.nAvgBytesPerSec;
        let block_align = value.nBlockAlign;
        let bits_per_sample = value.wBitsPerSample;
        let size = value.cbSize;
        Self {
            format_tag,
            channels,
            samples_per_sec,
            avg_bytes_per_sec,
            block_align,
            bits_per_sample,
            size,
        }
    }
}
