//! Process Loopback Capture で特定プロセス (ツリー) の音だけを wav に保存する最小検証。
//!
//! device_loopback.rs はデフォルトレンダーデバイス全体を loopback するので
//! 「特定アプリのみ」を分離できない。こちらは
//! ActivateAudioInterfaceAsync + AUDIOCLIENT_ACTIVATION_PARAMS で
//! 特定 PID のレンダーストリームを直接フックする。
//!
//! 注意: WASAPI 排他モードで再生されている音は本 API でも取れない (Windows の
//! 仕様)。排他モードのアプリを拾いたい場合は input_capture.rs を参照。

use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;
use std::ptr;
use std::sync::mpsc::{channel, Sender};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context as _, Result};
use clap::Parser;
use duration_str::parse_std;
use wav::{BitDepth, Header};
use windows::core::{implement, Interface, HRESULT};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Media::Audio::{
    eMultimedia, eRender, ActivateAudioInterfaceAsync, IAudioSessionControl2,
    IAudioSessionManager2, IActivateAudioInterfaceAsyncOperation,
    IActivateAudioInterfaceCompletionHandler, IActivateAudioInterfaceCompletionHandler_Impl,
    IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator,
    AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, AUDIOCLIENT_ACTIVATION_PARAMS,
    AUDIOCLIENT_ACTIVATION_PARAMS_0, AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
    AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS, PROCESS_LOOPBACK_MODE,
    PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE,
    PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE, VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
    WAVEFORMATEX,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows::core::PCWSTR;

// MS サンプル (Windows-classic-samples/ApplicationLoopback) 準拠の format。
// Process Loopback Capture でサポートされる組み合わせのうち、もっとも枯れているもの。
const WAVE_FORMAT_PCM: u16 = 0x0001;
const SAMPLE_RATE: u32 = 44_100;
const CHANNELS: u16 = 2;
const BITS_PER_SAMPLE: u16 = 16;
const VT_BLOB: u16 = 65;

#[derive(Parser, Debug)]
struct Cli {
    /// 出力 wav パス (省略時は録音せず診断のみ)
    #[clap(short, long)]
    output: Option<PathBuf>,
    /// 録音時間 (例: 10s, 1m)
    #[clap(short, long, default_value = "10s")]
    duration: String,
    /// 取得対象の実行ファイル名 (拡張子込み・大文字小文字無視)。
    /// 例: `chrome.exe`, `firefox.exe` 等。
    #[clap(short, long)]
    process: Option<String>,
    /// 指定すればプロセス名検索を無視して、この PID を target_process_id に渡す
    #[clap(long)]
    pid: Option<u32>,
    /// include: 指定 PID ツリーのみ取得 (デフォルト) / exclude: 指定 PID ツリー *以外* を取得
    #[clap(long, default_value = "include")]
    mode: LoopbackMode,
}

#[derive(clap::ValueEnum, Clone, Debug)]
enum LoopbackMode {
    Include,
    Exclude,
}

fn main() -> Result<()> {
    std::env::set_var("RUST_LOG", "INFO");
    env_logger::init();

    let cli = Cli::parse();
    let duration = parse_std(&cli.duration).map_err(|e| anyhow!("duration parse: {e}"))?;

    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok()? };

    log::info!("--- audio sessions on default render endpoint ---");
    if let Err(e) = unsafe { enumerate_sessions() } {
        log::warn!("session enumeration failed: {e:#}");
    }
    log::info!("--- end audio sessions ---");

    let pid = match (cli.pid, cli.process.as_deref()) {
        (Some(p), _) => p,
        (None, Some(name)) => find_root_pid_by_name(name)?
            .with_context(|| format!("running process not found: {name}"))?,
        (None, None) => bail!("--pid <PID> か --process <NAME> のどちらかを指定してください"),
    };
    let mode = match cli.mode {
        LoopbackMode::Include => PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
        LoopbackMode::Exclude => PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE,
    };
    log::info!("target pid = {pid} mode = {:?}", cli.mode);

    let buffer = unsafe { capture(pid, mode, duration)? };
    log::info!("captured {} bytes", buffer.len());

    if let Some(out) = &cli.output {
        write_wav(out, &buffer)?;
        log::info!("wrote {}", out.display());
    }

    unsafe { CoUninitialize() };
    Ok(())
}

/// 既定の再生エンドポイントに紐づくオーディオセッションを列挙し、各セッションを
/// 所有するプロセス PID と表示名をログに出す。対象アプリの音をどの PID で
/// 拾うべきかが分からないとき、ここで Windows 側の答えを直接見られる。
/// (排他モードのセッションはここには出ないので注意。)
unsafe fn enumerate_sessions() -> Result<()> {
    let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
    let device = enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia)?;
    let manager: IAudioSessionManager2 = device.Activate(CLSCTX_ALL, None)?;
    let sessions = manager.GetSessionEnumerator()?;
    let count = sessions.GetCount()?;
    log::info!("audio session count = {count}");
    for i in 0..count {
        let control = sessions.GetSession(i)?;
        let control2: IAudioSessionControl2 = control.cast()?;
        let pid = control2.GetProcessId().unwrap_or(0);
        let display = control
            .GetDisplayName()
            .ok()
            .and_then(|w| w.to_string().ok())
            .unwrap_or_default();
        let session_id = control2
            .GetSessionIdentifier()
            .ok()
            .and_then(|w| w.to_string().ok())
            .unwrap_or_default();
        log::info!("  session[{i}] pid={pid} display=\"{display}\" id=\"{session_id}\"");
    }
    Ok(())
}

/// 対象のプロセス名のうち、親が同名でないもの (= ツリーのルート) を返す。
/// 普通のアプリは複数の子プロセスを生やすので、ルートを INCLUDE_TARGET_PROCESS_TREE で
/// 拾えば配下のレンダーも全部捕まる。
fn find_root_pid_by_name(name: &str) -> Result<Option<u32>> {
    let target = name.to_ascii_lowercase();

    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)?;
        let _guard = HandleGuard(snapshot);

        let mut entries: Vec<(u32, u32, String)> = Vec::new();
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                let len = entry
                    .szExeFile
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(entry.szExeFile.len());
                let exe = String::from_utf16_lossy(&entry.szExeFile[..len]).to_ascii_lowercase();
                entries.push((entry.th32ProcessID, entry.th32ParentProcessID, exe));
                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }

        let matches: Vec<&(u32, u32, String)> =
            entries.iter().filter(|(_, _, name)| *name == target).collect();
        log::info!("found {} {name} processes", matches.len());
        for (pid, ppid, _) in &matches {
            log::info!("  pid={pid} ppid={ppid}");
        }
        if matches.is_empty() {
            return Ok(None);
        }
        let parent_is_same = |ppid: u32| {
            entries
                .iter()
                .any(|(pid, _, n)| *pid == ppid && *n == target)
        };
        if let Some((pid, _, _)) = matches.iter().find(|(_, ppid, _)| !parent_is_same(*ppid)) {
            return Ok(Some(*pid));
        }
        Ok(Some(matches[0].0))
    }
}

struct HandleGuard(HANDLE);
impl Drop for HandleGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

#[implement(IActivateAudioInterfaceCompletionHandler)]
struct ActivationHandler {
    tx: Sender<()>,
}

impl IActivateAudioInterfaceCompletionHandler_Impl for ActivationHandler {
    fn ActivateCompleted(
        &self,
        _op: Option<&IActivateAudioInterfaceAsyncOperation>,
    ) -> windows::core::Result<()> {
        let _ = self.tx.send(());
        Ok(())
    }
}

/// PROPVARIANT(VT_BLOB) の生レイアウト。windows crate 側に VT_BLOB の
/// セーフコンストラクタが無いので、x64 PROPVARIANT (24 byte) を手で組み立てる。
#[repr(C)]
struct PropVariantBlob {
    vt: u16,
    reserved1: u16,
    reserved2: u16,
    reserved3: u16,
    blob_size: u32,
    _pad: u32,
    blob_ptr: *mut u8,
}

unsafe fn capture(pid: u32, mode: PROCESS_LOOPBACK_MODE, duration: Duration) -> Result<Vec<u8>> {
    let mut activation_params = AUDIOCLIENT_ACTIVATION_PARAMS {
        ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
        Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
            ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                TargetProcessId: pid,
                ProcessLoopbackMode: mode,
            },
        },
    };
    let propvariant = PropVariantBlob {
        vt: VT_BLOB,
        reserved1: 0,
        reserved2: 0,
        reserved3: 0,
        blob_size: std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
        _pad: 0,
        blob_ptr: &mut activation_params as *mut _ as *mut u8,
    };

    let (tx, rx) = channel::<()>();
    let handler: IActivateAudioInterfaceCompletionHandler =
        ActivationHandler { tx }.into();

    let async_op: IActivateAudioInterfaceAsyncOperation = ActivateAudioInterfaceAsync(
        VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
        &IAudioClient::IID,
        Some(&propvariant as *const _ as *const _),
        &handler,
    )?;

    rx.recv_timeout(Duration::from_secs(5))
        .context("activation timed out")?;

    let mut hr = HRESULT(0);
    let mut iface: Option<windows::core::IUnknown> = None;
    async_op.GetActivateResult(&mut hr, &mut iface)?;
    if hr.is_err() {
        bail!("activation failed: 0x{:08x}", hr.0);
    }
    let client: IAudioClient = iface
        .ok_or_else(|| anyhow!("no audio client returned"))?
        .cast()?;

    let format = WAVEFORMATEX {
        wFormatTag: WAVE_FORMAT_PCM,
        nChannels: CHANNELS,
        nSamplesPerSec: SAMPLE_RATE,
        nAvgBytesPerSec: SAMPLE_RATE * (CHANNELS as u32) * (BITS_PER_SAMPLE as u32 / 8),
        nBlockAlign: CHANNELS * (BITS_PER_SAMPLE / 8),
        wBitsPerSample: BITS_PER_SAMPLE,
        cbSize: 0,
    };

    // Process Loopback Capture は MS サンプル (Windows-classic-samples/ApplicationLoopback)
    // の通り EVENTCALLBACK + 明示的なバッファ長/周期で初期化しないと
    // フレームは流れるがデータが入らない (= ゼロ) 報告がある。
    const AUDCLNT_STREAMFLAGS_EVENTCALLBACK: u32 = 0x00040000;
    const BUF_DUR_HNS: i64 = 200_000; // 20ms
    const PERIOD_HNS: i64 = 0x20000; // ≒13ms (MS サンプル準拠)
    client
        .Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
            BUF_DUR_HNS,
            PERIOD_HNS,
            &format,
            None,
        )
        .context("IAudioClient::Initialize failed")?;

    let event = CreateEventW(None, false, false, PCWSTR::null())
        .context("CreateEventW failed")?;
    client
        .SetEventHandle(event)
        .context("SetEventHandle failed")?;

    let capture_client: IAudioCaptureClient = client.GetService()?;
    client.Start()?;

    let block_align = format.nBlockAlign as u32;
    let mut buffer_all = Vec::<u8>::with_capacity((format.nAvgBytesPerSec as usize) * 10);
    let started_at = Instant::now();

    let mut total_packets: u64 = 0;
    let mut silent_packets: u64 = 0;
    let mut total_frames: u64 = 0;
    let mut peak: f32 = 0.0;
    let mut first_dump_done = false;
    const AUDCLNT_BUFFERFLAGS_SILENT: u32 = 0x2;

    while started_at.elapsed() < duration {
        // EVENTCALLBACK モード: バッファ準備完了まで待つ
        let _ = WaitForSingleObject(event, 100);

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
            continue;
        }
        total_packets += 1;
        total_frames += stored_frames as u64;
        let len = (stored_frames * block_align) as usize;
        if (flags & AUDCLNT_BUFFERFLAGS_SILENT) != 0 || buffer_ptr.is_null() {
            silent_packets += 1;
            buffer_all.extend(std::iter::repeat(0u8).take(len));
        } else {
            let slice = std::slice::from_raw_parts(buffer_ptr, len);
            if !first_dump_done {
                let dump_len = slice.len().min(32);
                let hex: Vec<String> =
                    slice[..dump_len].iter().map(|b| format!("{b:02x}")).collect();
                log::info!(
                    "first packet bytes ({} of {} total): {}",
                    dump_len,
                    len,
                    hex.join(" ")
                );
                first_dump_done = true;
            }
            // ピーク振幅 (i16 LE サンプル前提, 0.0..1.0 に正規化)
            for chunk in slice.chunks_exact(2) {
                let v = (i16::from_le_bytes([chunk[0], chunk[1]]).abs() as f32) / 32768.0;
                if v > peak {
                    peak = v;
                }
            }
            buffer_all.extend_from_slice(slice);
        }
        capture_client.ReleaseBuffer(stored_frames)?;
    }

    client.Stop()?;
    log::info!(
        "packets total={total_packets} silent={silent_packets} frames={total_frames} peak={peak:.6}"
    );
    Ok(buffer_all)
}

fn write_wav(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    let header = Header::new(
        wav::WAV_FORMAT_PCM,
        CHANNELS,
        SAMPLE_RATE,
        BITS_PER_SAMPLE,
    );
    let samples: Vec<i16> = bytes
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect();
    let payload = BitDepth::Sixteen(samples);
    let mut writer = BufWriter::new(File::create(path)?);
    wav::write(header, &payload, &mut writer)?;
    Ok(())
}
