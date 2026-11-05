//! WASAPI (shared mode, event-driven) devices and streams.
//!
//! Every stream asks for float32 48 kHz (stereo out, mono in) and lets Windows convert to the
//! device's own format (`AUTOCONVERTPCM | SRC_DEFAULT_QUALITY`), which is what the DirectSound host
//! did for the Python app. Device names are the endpoints' friendly names ("CABLE Input
//! (VB-Audio Virtual Cable)"), the same strings DirectSound showed, so saved settings still match.
//!
//! Each stream runs on its own thread, registered with MMCSS ("Pro Audio"), and talks to the rest
//! of the app only through the lock-free `OutputMixer` / `MicRing`. All `unsafe` here is COM and
//! the raw device buffer WASAPI hands out for exactly `frames` frames.

use super::mixer::{MicRing, OutputMixer, SR};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::*;
use windows::Win32::Media::KernelStreaming::WAVE_FORMAT_EXTENSIBLE;
use windows::Win32::Media::Multimedia::KSDATAFORMAT_SUBTYPE_IEEE_FLOAT;
use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PropVariantToStringAlloc};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ};
use windows::Win32::System::Threading::{AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW, CreateEventW, WaitForSingleObject};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Input,
    Output,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
}

/// COM for the calling thread, released when dropped.
pub struct ComScope(bool);

impl ComScope {
    pub fn mta() -> ComScope {
        // SAFETY: plain COM initialisation; balanced by Drop.
        let ok = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
        ComScope(ok)
    }
}

impl Drop for ComScope {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: balances the successful CoInitializeEx above.
            unsafe { CoUninitialize() };
        }
    }
}

fn enumerator() -> windows::core::Result<IMMDeviceEnumerator> {
    // SAFETY: COM is initialised on every thread that calls this (UI thread: STA, audio: MTA).
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
}

unsafe fn pwstr_take(p: PWSTR) -> String {
    let s = unsafe { p.to_string() }.unwrap_or_default();
    unsafe { CoTaskMemFree(Some(p.0 as *const _)) };
    s
}

fn device_info(dev: &IMMDevice) -> Option<DeviceInfo> {
    // SAFETY: COM calls on a valid device; the strings are copied and their memory freed.
    unsafe {
        let id = pwstr_take(dev.GetId().ok()?);
        let store = dev.OpenPropertyStore(STGM_READ).ok()?;
        let mut v = store.GetValue(&PKEY_Device_FriendlyName).ok()?;
        let name = PropVariantToStringAlloc(&v).ok().map(|p| pwstr_take(p)).unwrap_or_default();
        let _ = PropVariantClear(&mut v);
        Some(DeviceInfo { id, name })
    }
}

fn flow(kind: Kind) -> EDataFlow {
    match kind {
        Kind::Input => eCapture,
        Kind::Output => eRender,
    }
}

/// Active devices of one kind, in Windows' order.
pub fn list(kind: Kind) -> Vec<DeviceInfo> {
    let Ok(en) = enumerator() else { return Vec::new() };
    // SAFETY: COM enumeration calls on a valid enumerator.
    unsafe {
        let Ok(col) = en.EnumAudioEndpoints(flow(kind), DEVICE_STATE_ACTIVE) else { return Vec::new() };
        let n = col.GetCount().unwrap_or(0);
        let mut out: Vec<DeviceInfo> = Vec::new();
        for i in 0..n {
            if let Some(d) = col.Item(i).ok().as_ref().and_then(device_info) {
                if !out.iter().any(|o| o.name == d.name) {
                    out.push(d);
                }
            }
        }
        out
    }
}

/// The Windows default device of a kind.
pub fn default_device(kind: Kind) -> Option<DeviceInfo> {
    let en = enumerator().ok()?;
    // SAFETY: COM call on a valid enumerator.
    let dev = unsafe { en.GetDefaultAudioEndpoint(flow(kind), eConsole) }.ok()?;
    device_info(&dev)
}

fn format(channels: u16) -> WAVEFORMATEXTENSIBLE {
    let mut f = WAVEFORMATEXTENSIBLE::default();
    f.Format.wFormatTag = WAVE_FORMAT_EXTENSIBLE as u16;
    f.Format.nChannels = channels;
    f.Format.nSamplesPerSec = SR;
    f.Format.wBitsPerSample = 32;
    f.Format.nBlockAlign = channels * 4;
    f.Format.nAvgBytesPerSec = SR * channels as u32 * 4;
    f.Format.cbSize = 22;
    f.Samples.wValidBitsPerSample = 32;
    f.dwChannelMask = if channels == 1 { 0x4 } else { 0x3 };
    f.SubFormat = KSDATAFORMAT_SUBTYPE_IEEE_FLOAT;
    f
}

struct Event(HANDLE);
impl Drop for Event {
    fn drop(&mut self) {
        // SAFETY: we own the handle.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

struct Mmcss(HANDLE);
impl Mmcss {
    fn pro_audio() -> Option<Mmcss> {
        let mut idx = 0u32;
        // SAFETY: registers this thread with MMCSS; reverted in Drop.
        unsafe { AvSetMmThreadCharacteristicsW(windows::core::w!("Pro Audio"), &mut idx) }.ok().map(Mmcss)
    }
}
impl Drop for Mmcss {
    fn drop(&mut self) {
        // SAFETY: the handle came from AvSetMmThreadCharacteristicsW on this thread.
        unsafe {
            let _ = AvRevertMmThreadCharacteristics(self.0);
        }
    }
}

/// Counters a stream thread keeps for the log and the diagnostics.
#[derive(Default)]
pub struct StreamStats {
    /// the device buffer had run dry when we woke up (a dropout)
    pub underflows: AtomicU64,
    /// capture: WASAPI flagged a discontinuity (samples lost)
    pub overflows: AtomicU64,
    /// the device went away / failed while running
    pub failed: AtomicBool,
}

/// A running stream thread; stops and joins when dropped.
pub struct Stream {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    pub stats: Arc<StreamStats>,
    pub name: String,
}

impl Drop for Stream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn find_device(en: &IMMDeviceEnumerator, id: &str) -> windows::core::Result<IMMDevice> {
    let wide: Vec<u16> = id.encode_utf16().chain(Some(0)).collect();
    // SAFETY: wide is a NUL-terminated UTF-16 string alive for the call.
    unsafe { en.GetDevice(PCWSTR(wide.as_ptr())) }
}

const HNS_PER_MS: i64 = 10_000;

/// Start an output stream that `mixer` fills. `on_fail` runs (on the stream thread) if the
/// device fails later, e.g. it's unplugged.
pub fn start_output(dev: &DeviceInfo, mut mixer: OutputMixer, on_fail: Box<dyn Fn() + Send>) -> Result<Stream, String> {
    let stop = Arc::new(AtomicBool::new(false));
    let stats = Arc::new(StreamStats::default());
    let (tx, rx) = mpsc::channel::<Result<(), String>>();
    let id = dev.id.clone();
    let (stop2, stats2) = (stop.clone(), stats.clone());
    let thread = std::thread::Builder::new()
        .name(format!("audio out {}", dev.name))
        .spawn(move || {
            let _com = ComScope::mta();
            let r = (|| -> windows::core::Result<(IAudioClient, IAudioRenderClient, Event, u32)> {
                let en = enumerator()?;
                let device = find_device(&en, &id)?;
                // SAFETY: COM activation and initialisation with a valid format struct.
                unsafe {
                    let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
                    let fmt = format(2);
                    client.Initialize(
                        AUDCLNT_SHAREMODE_SHARED,
                        AUDCLNT_STREAMFLAGS_EVENTCALLBACK | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                        20 * HNS_PER_MS,
                        0,
                        &fmt as *const _ as *const WAVEFORMATEX,
                        None,
                    )?;
                    let ev = Event(CreateEventW(None, false, false, None)?);
                    client.SetEventHandle(ev.0)?;
                    let size = client.GetBufferSize()?;
                    let render: IAudioRenderClient = client.GetService()?;
                    // start from silence
                    let _ = render.GetBuffer(size)?;
                    render.ReleaseBuffer(size, AUDCLNT_BUFFERFLAGS_SILENT.0 as u32)?;
                    client.Start()?;
                    Ok((client, render, ev, size))
                }
            })();
            let (client, render, ev, size) = match r {
                Ok(x) => {
                    let _ = tx.send(Ok(()));
                    x
                }
                Err(e) => {
                    let _ = tx.send(Err(e.message().to_string()));
                    return;
                }
            };
            let _mm = Mmcss::pro_audio();
            let mut primed = false;
            while !stop2.load(Ordering::Relaxed) {
                // SAFETY: waiting on our own event handle.
                let w = unsafe { WaitForSingleObject(ev.0, 200) };
                if w != WAIT_OBJECT_0 {
                    continue;
                }
                // SAFETY: COM calls on the live client; the buffer pointer is valid for `avail`
                // frames of 2 x f32 until ReleaseBuffer, and nothing else aliases it.
                let res = unsafe {
                    (|| -> windows::core::Result<()> {
                        let padding = client.GetCurrentPadding()?;
                        if primed && padding == 0 {
                            stats2.underflows.fetch_add(1, Ordering::Relaxed);
                        }
                        primed = true;
                        let avail = size - padding;
                        if avail == 0 {
                            return Ok(());
                        }
                        let p = render.GetBuffer(avail)?;
                        let buf = std::slice::from_raw_parts_mut(p as *mut f32, avail as usize * 2);
                        mixer.render(buf);
                        render.ReleaseBuffer(avail, 0)
                    })()
                };
                if res.is_err() {
                    stats2.failed.store(true, Ordering::Relaxed);
                    on_fail();
                    break;
                }
            }
            // SAFETY: stopping our own client.
            unsafe {
                let _ = client.Stop();
            }
        })
        .map_err(|e| e.to_string())?;
    match rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(())) => Ok(Stream { stop, thread: Some(thread), stats, name: dev.name.clone() }),
        Ok(Err(e)) => {
            let _ = thread.join();
            Err(e)
        }
        Err(_) => {
            stop.store(true, Ordering::Relaxed);
            Err("the audio device didn't respond".into())
        }
    }
}

/// Start capturing a mic (mono) into `ring`.
pub fn start_input(dev: &DeviceInfo, ring: Arc<MicRing>, on_fail: Box<dyn Fn() + Send>) -> Result<Stream, String> {
    start_capture(dev, 1, Box::new(move |x: &[f32]| ring.write(x)), on_fail)
}

/// Capture `channels` interleaved float channels; `sink` gets every packet (on the capture
/// thread, so it must not block).
pub fn start_capture(dev: &DeviceInfo, channels: u16, mut sink: Box<dyn FnMut(&[f32]) + Send>, on_fail: Box<dyn Fn() + Send>) -> Result<Stream, String> {
    let stop = Arc::new(AtomicBool::new(false));
    let stats = Arc::new(StreamStats::default());
    let (tx, rx) = mpsc::channel::<Result<(), String>>();
    let id = dev.id.clone();
    let (stop2, stats2) = (stop.clone(), stats.clone());
    let thread = std::thread::Builder::new()
        .name(format!("audio in {}", dev.name))
        .spawn(move || {
            let _com = ComScope::mta();
            let r = (|| -> windows::core::Result<(IAudioClient, IAudioCaptureClient, Event)> {
                let en = enumerator()?;
                let device = find_device(&en, &id)?;
                // SAFETY: COM activation and initialisation with a valid format struct.
                unsafe {
                    let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
                    let fmt = format(channels);
                    client.Initialize(
                        AUDCLNT_SHAREMODE_SHARED,
                        AUDCLNT_STREAMFLAGS_EVENTCALLBACK | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                        20 * HNS_PER_MS,
                        0,
                        &fmt as *const _ as *const WAVEFORMATEX,
                        None,
                    )?;
                    let ev = Event(CreateEventW(None, false, false, None)?);
                    client.SetEventHandle(ev.0)?;
                    let cap: IAudioCaptureClient = client.GetService()?;
                    client.Start()?;
                    Ok((client, cap, ev))
                }
            })();
            let (client, cap, ev) = match r {
                Ok(x) => {
                    let _ = tx.send(Ok(()));
                    x
                }
                Err(e) => {
                    let _ = tx.send(Err(e.message().to_string()));
                    return;
                }
            };
            let _mm = Mmcss::pro_audio();
            let zeros = vec![0.0f32; SR as usize * channels as usize];
            while !stop2.load(Ordering::Relaxed) {
                // SAFETY: waiting on our own event handle.
                let w = unsafe { WaitForSingleObject(ev.0, 200) };
                if w != WAIT_OBJECT_0 {
                    continue;
                }
                // SAFETY: COM calls on the live client; the packet pointer is valid for `frames`
                // frames of `channels` f32 samples until ReleaseBuffer.
                let res = unsafe {
                    (|| -> windows::core::Result<()> {
                        loop {
                            let n = cap.GetNextPacketSize()?;
                            if n == 0 {
                                return Ok(());
                            }
                            let mut p: *mut u8 = std::ptr::null_mut();
                            let mut frames = 0u32;
                            let mut flags = 0u32;
                            cap.GetBuffer(&mut p, &mut frames, &mut flags, None, None)?;
                            if flags & AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 as u32 != 0 {
                                stats2.overflows.fetch_add(1, Ordering::Relaxed);
                            }
                            let total = frames as usize * channels as usize;
                            if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || p.is_null() {
                                let mut left = total;
                                while left > 0 {
                                    let k = left.min(zeros.len());
                                    sink(&zeros[..k]);
                                    left -= k;
                                }
                            } else {
                                sink(std::slice::from_raw_parts(p as *const f32, total));
                            }
                            cap.ReleaseBuffer(frames)?;
                        }
                    })()
                };
                if res.is_err() {
                    stats2.failed.store(true, Ordering::Relaxed);
                    on_fail();
                    break;
                }
            }
            // SAFETY: stopping our own client.
            unsafe {
                let _ = client.Stop();
            }
        })
        .map_err(|e| e.to_string())?;
    match rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(())) => Ok(Stream { stop, thread: Some(thread), stats, name: dev.name.clone() }),
        Ok(Err(e)) => {
            let _ = thread.join();
            Err(e)
        }
        Err(_) => {
            stop.store(true, Ordering::Relaxed);
            Err("the recording device didn't respond".into())
        }
    }
}

