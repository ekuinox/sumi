//! WASAPI shared-mode capture from a named input device, running on a background
//! thread and exposing the most recent N samples through a shared mutex.
//!
//! Typical target is a hardware-loopback channel on the audio interface
//! (e.g. `Mix 3/4`), which carries whatever the interface is rendering even
//! when an app holds the device in WASAPI exclusive mode. See
//! `examples/input_capture.rs` for the standalone
//! validation of this path.
//!
//! COM initialization is kept entirely on the capture thread. The main thread
//! must NOT init COM in MTA mode, otherwise winit's drag-and-drop OleInitialize
//! conflicts with `RPC_E_CHANGED_MODE`.

use std::collections::VecDeque;
use std::ptr;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, bail, Context as _, Result};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Media::Audio::{
    eCapture, IAudioCaptureClient, IAudioClient, IMMDevice, IMMDeviceEnumerator,
    MMDeviceEnumerator, AUDCLNT_SHAREMODE_SHARED, DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ,
};

const WAVE_FORMAT_IEEE_FLOAT: u16 = 0x0003;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;
const AUDCLNT_BUFFERFLAGS_SILENT: u32 = 0x2;

#[derive(Debug, Clone, Copy)]
pub struct StreamFormat {
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
    pub is_float: bool,
}

pub struct SampleBuffer {
    inner: Mutex<VecDeque<f32>>,
    capacity: usize,
}

impl SampleBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Mutex::new(VecDeque::with_capacity(capacity)),
            capacity,
        }
    }

    pub fn push_many(&self, samples: &[f32]) {
        let mut buf = self.inner.lock().unwrap();
        buf.extend(samples);
        let excess = buf.len().saturating_sub(self.capacity);
        if excess > 0 {
            buf.drain(..excess);
        }
    }

    pub fn read_latest(&self, out: &mut [f32]) {
        let buf = self.inner.lock().unwrap();
        let n = out.len();
        if buf.len() >= n {
            let start = buf.len() - n;
            for (i, s) in buf.iter().skip(start).enumerate() {
                out[i] = *s;
            }
        } else {
            let pad = n - buf.len();
            out[..pad].fill(0.0);
            for (i, s) in buf.iter().enumerate() {
                out[pad + i] = *s;
            }
        }
    }
}

pub struct CaptureHandle {
    pub samples: Arc<SampleBuffer>,
    pub format: StreamFormat,
}

pub fn spawn_capture(
    device_name_substring: String,
    buffer_capacity: usize,
) -> Result<CaptureHandle> {
    let samples = Arc::new(SampleBuffer::new(buffer_capacity));
    let samples_for_thread = samples.clone();
    let (fmt_tx, fmt_rx) = mpsc::channel::<Result<StreamFormat>>();

    thread::Builder::new()
        .name("chryth-audio".into())
        .spawn(move || {
            let res = unsafe { capture_thread_main(device_name_substring, samples_for_thread, fmt_tx.clone()) };
            if let Err(e) = res {
                // If we never sent the format back, the main thread is still
                // blocked in recv — relay the error there.
                let _ = fmt_tx.send(Err(e));
            }
        })
        .context("spawn capture thread")?;

    let format = fmt_rx
        .recv()
        .context("capture thread exited before reporting format")??;
    Ok(CaptureHandle { samples, format })
}

unsafe fn capture_thread_main(
    needle: String,
    samples: Arc<SampleBuffer>,
    fmt_tx: mpsc::Sender<Result<StreamFormat>>,
) -> Result<()> {
    CoInitializeEx(None, COINIT_MULTITHREADED)
        .ok()
        .context("CoInitializeEx")?;

    let device = find_capture_device(&needle)?;
    let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
    let raw_fmt = client.GetMixFormat()?;
    let fmt = *raw_fmt;
    let is_float = fmt.wFormatTag == WAVE_FORMAT_IEEE_FLOAT
        || (fmt.wFormatTag == WAVE_FORMAT_EXTENSIBLE && fmt.wBitsPerSample == 32);
    let bytes_per_sample = (fmt.wBitsPerSample / 8) as usize;
    let block_align = fmt.nBlockAlign as usize;
    let channels = fmt.nChannels as usize;

    let buffered = Duration::from_millis(200);
    client.Initialize(
        AUDCLNT_SHAREMODE_SHARED,
        0,
        buffered.as_micros() as i64 * 10,
        0,
        raw_fmt,
        None,
    )?;

    let stream_fmt = StreamFormat {
        sample_rate: fmt.nSamplesPerSec,
        channels: fmt.nChannels,
        bits_per_sample: fmt.wBitsPerSample,
        is_float,
    };
    fmt_tx
        .send(Ok(stream_fmt))
        .map_err(|_| anyhow!("main thread dropped before format report"))?;

    let capture_client: IAudioCaptureClient = client.GetService()?;
    client.Start()?;

    let mut scratch: Vec<f32> = Vec::with_capacity(4096);
    loop {
        let mut buffer_ptr: *mut u8 = ptr::null_mut();
        let mut frames: u32 = 0;
        let mut flags: u32 = 0;
        capture_client.GetBuffer(&mut buffer_ptr, &mut frames, &mut flags, None, None)?;
        if frames == 0 {
            thread::sleep(Duration::from_millis(2));
            continue;
        }
        scratch.clear();
        scratch.reserve(frames as usize);
        let len_bytes = (frames as usize) * block_align;
        let is_silent = (flags & AUDCLNT_BUFFERFLAGS_SILENT) != 0 || buffer_ptr.is_null();
        if is_silent {
            scratch.extend(std::iter::repeat(0.0_f32).take(frames as usize));
        } else {
            let raw = std::slice::from_raw_parts(buffer_ptr, len_bytes);
            for frame in raw.chunks_exact(block_align) {
                let mut sum = 0.0_f32;
                for c in 0..channels {
                    let off = c * bytes_per_sample;
                    let s = decode_sample(&frame[off..off + bytes_per_sample], is_float);
                    sum += s;
                }
                scratch.push(sum / channels as f32);
            }
        }
        samples.push_many(&scratch);
        capture_client.ReleaseBuffer(frames)?;
    }
}

unsafe fn find_capture_device(needle_substr: &str) -> Result<IMMDevice> {
    let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
    let collection = enumerator.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE)?;
    let count = collection.GetCount()?;
    let needle = needle_substr.to_ascii_lowercase();
    for i in 0..count {
        let dev = collection.Item(i)?;
        let store = dev.OpenPropertyStore(STGM_READ)?;
        let value = store.GetValue(&PKEY_Device_FriendlyName)?;
        let name = value.to_string();
        if name.to_ascii_lowercase().contains(&needle) {
            log::info!("audio: using device \"{name}\"");
            return Ok(dev);
        }
    }
    bail!("no capture device matching {needle_substr:?}");
}

fn decode_sample(bytes: &[u8], is_float: bool) -> f32 {
    if is_float && bytes.len() == 4 {
        f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
    } else if bytes.len() == 2 {
        (i16::from_le_bytes([bytes[0], bytes[1]]) as f32) / 32768.0
    } else if bytes.len() == 4 {
        (i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f32) / 2_147_483_648.0
    } else {
        0.0
    }
}
