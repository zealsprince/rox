//! Exclusive output on Windows: WASAPI in `AUDCLNT_SHAREMODE_EXCLUSIVE`, at the
//! file's own rate and format. Shared WASAPI resamples everything, and cpal
//! has no exclusive path, so this talks to the API directly.
//!
//! Same shape as `alsa.rs`. COM interfaces are apartment-bound and not `Send`,
//! so everything is created, used, and released on one writer thread; `open`
//! waits for it to report what it negotiated.
//!
//! The negotiation math sits above the FFI so it's tested on every platform.

#[cfg(target_os = "windows")]
pub use platform::{devices, open};

/// 100 ns ticks.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const HNS_PER_SEC: f64 = 10_000_000.0;

/// Before a file is decoded; the pump reopens at the file's rate.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const DEFAULT_RATE: u32 = 48000;

/// 100 ms. [`PERIODS`] of them must stay under what the 500 ms ring keeps
/// stocked, and the writer only sees the stop flag once per wake.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const MAX_PERIOD_HNS: i64 = 1_000_000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
enum Sample {
    F32,
    I32,
    I16,
}

/// One rung of the format ladder.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
struct Candidate {
    sample: Sample,
    bits: u16,
    valid_bits: u16,
    float: bool,
    /// Spelled like the ALSA backend's, so one setting means one thing everywhere.
    ///
    /// [`Negotiated`]: super::Negotiated
    name: &'static str,
}

/// Best first. "24-bit" on Windows is 24 valid bits in a 32-bit container,
/// written as a plain `i32` and reported as `s32`. No packed 24-bit.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const FORMATS: &[Candidate] = &[
    Candidate {
        sample: Sample::F32,
        bits: 32,
        valid_bits: 32,
        float: true,
        name: "f32",
    },
    Candidate {
        sample: Sample::I32,
        bits: 32,
        valid_bits: 24,
        float: false,
        name: "s32",
    },
    Candidate {
        sample: Sample::I16,
        bits: 16,
        valid_bits: 16,
        float: false,
        name: "s16",
    },
];

/// A named format the device takes wins; otherwise the best it has.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn pick_format(
    want: Option<&str>,
    mut supported: impl FnMut(&Candidate) -> bool,
) -> Option<&'static Candidate> {
    if let Some(named) = want.and_then(|want| FORMATS.iter().find(|c| c.name == want))
        && supported(named)
    {
        return Some(named);
    }
    FORMATS.iter().find(|c| supported(c))
}

/// Push mode takes a buffer deeper than its period; four, like ALSA, so a
/// late timer wake doesn't run it dry.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const PERIODS: i64 = 4;

/// The driver minimum is a hard floor: asking below it fails the claim.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn period_hns(want_ms: Option<f64>, default_hns: i64, min_hns: i64) -> i64 {
    let want = want_ms
        .map(|ms| (ms / 1000.0 * HNS_PER_SEC).round() as i64)
        .unwrap_or(default_hns);
    want.clamp(min_hns.max(1), MAX_PERIOD_HNS.max(min_hns.max(1)))
}

/// Capped at 400 ms, which the 500 ms ring can keep stocked.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn buffer_hns(period: i64) -> i64 {
    period.saturating_mul(PERIODS)
}

/// Half a period, off the buffer the driver actually granted.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn wake_frames(buffer_frames: u32) -> u32 {
    (buffer_frames / (PERIODS as u32 * 2)).max(1)
}

/// Turns `GetBufferSize`'s aligned frame count back into a duration. The
/// half-tick rounding is Microsoft's sample's; rounding down comes out a
/// frame short and the driver refuses again.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn aligned_hns(frames: u32, rate: u32) -> i64 {
    (HNS_PER_SEC / rate as f64 * frames as f64 + 0.5) as i64
}

#[cfg(target_os = "windows")]
mod platform {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{self, Receiver, Sender};
    use std::thread::JoinHandle;
    use std::time::Duration;

    use cpal::{FromSample, SizedSample};
    use rtrb::{Consumer, Producer};
    use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
    use windows::Win32::Foundation::{CloseHandle, HANDLE, S_OK};
    use windows::Win32::Media::Audio::{
        AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED, AUDCLNT_E_DEVICE_INVALIDATED, AUDCLNT_E_NOT_INITIALIZED,
        AUDCLNT_E_OUT_OF_ORDER, AUDCLNT_E_RESOURCES_INVALIDATED, AUDCLNT_E_SERVICE_NOT_RUNNING,
        AUDCLNT_SHAREMODE_EXCLUSIVE, DEVICE_STATE_ACTIVE, IAudioClient, IAudioRenderClient,
        IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator, WAVEFORMATEX, WAVEFORMATEXTENSIBLE,
        WAVEFORMATEXTENSIBLE_0, eConsole, eRender,
    };
    use windows::Win32::Media::KernelStreaming::{
        KSDATAFORMAT_SUBTYPE_PCM, SPEAKER_BACK_LEFT, SPEAKER_BACK_RIGHT, SPEAKER_FRONT_CENTER,
        SPEAKER_FRONT_LEFT, SPEAKER_FRONT_RIGHT, SPEAKER_LOW_FREQUENCY, SPEAKER_SIDE_LEFT,
        SPEAKER_SIDE_RIGHT, WAVE_FORMAT_EXTENSIBLE,
    };
    use windows::Win32::Media::Multimedia::KSDATAFORMAT_SUBTYPE_IEEE_FLOAT;
    use windows::Win32::System::Com::StructuredStorage::PropVariantClear;
    use windows::Win32::System::Com::{
        CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
        CoUninitialize, STGM_READ,
    };
    use windows::Win32::System::Threading::{
        CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CreateWaitableTimerExW, INFINITE, SetWaitableTimer,
        TIMER_ALL_ACCESS, WaitForSingleObject,
    };
    use windows::Win32::System::Variant::VT_LPWSTR;
    use windows::core::{Error, HRESULT, PCWSTR};

    use super::super::{Device, Mode, Negotiated, OpenOutput, OutputStream, Request, fill, rings};
    use super::{
        Candidate, DEFAULT_RATE, Sample, aligned_hns, buffer_hns, period_hns, pick_format,
        wake_frames,
    };
    use crate::shared::Shared;

    /// Consecutive write faults before the device counts as gone.
    const MAX_FAULTS: u32 = 4;

    /// MTA COM for this thread's lifetime. Every caller owns its thread; never
    /// borrow the caller's apartment, since the UI thread is an STA.
    struct Com;

    impl Com {
        fn new() -> Result<Self, String> {
            unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
                .ok()
                .map_err(|e| format!("com init: {e}"))?;
            Ok(Com)
        }
    }

    impl Drop for Com {
        fn drop(&mut self) {
            unsafe { CoUninitialize() };
        }
    }

    /// `thread::sleep` rides the ~15.6 ms system tick, three periods late at
    /// 10 ms. A high-resolution waitable timer (Windows 10 1803+) fires on time;
    /// without one we sleep and lean on the buffer's spare periods.
    enum Ticker {
        Timer(HANDLE),
        Sleep(Duration),
    }

    impl Ticker {
        fn new(interval: Duration) -> Self {
            let handle = unsafe {
                CreateWaitableTimerExW(
                    None,
                    PCWSTR::null(),
                    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                    TIMER_ALL_ACCESS.0,
                )
            };
            if let Ok(handle) = handle {
                // Negative due time is relative, in reference time; the period must be at
                // least 1 ms or the timer fires once.
                let due = -((interval.as_nanos() / 100) as i64);
                let period = (interval.as_millis() as i32).max(1);
                if unsafe { SetWaitableTimer(handle, &due, period, None, None, false) }.is_ok() {
                    return Ticker::Timer(handle);
                }
                unsafe {
                    let _ = CloseHandle(handle);
                };
            }
            Ticker::Sleep(interval)
        }

        fn wait(&self) {
            match self {
                // Periodic timer: a tick or an abandon both mean check the padding.
                Ticker::Timer(handle) => unsafe {
                    WaitForSingleObject(*handle, INFINITE);
                },
                Ticker::Sleep(interval) => std::thread::sleep(*interval),
            }
        }
    }

    impl Drop for Ticker {
        fn drop(&mut self) {
            if let Ticker::Timer(handle) = self {
                unsafe {
                    let _ = CloseHandle(*handle);
                };
            }
        }
    }

    /// Dropping it stops audio and hands the device back.
    struct Claim {
        stop: Arc<AtomicBool>,
        writer: Option<JoinHandle<()>>,
    }

    impl OutputStream for Claim {}

    impl Drop for Claim {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            // Waits half a period at worst. Join, don't detach: the thread owns every
            // COM interface and would still hold the endpoint.
            if let Some(writer) = self.writer.take() {
                let _ = writer.join();
            }
        }
    }

    /// Reported once negotiated, so `open` can size the rings.
    struct Ready {
        negotiated: Negotiated,
        rate: u32,
    }

    struct Feed {
        ring: Consumer<f32>,
        tap: Producer<f32>,
    }

    /// All COM; none of it leaves the thread.
    struct Claimed {
        client: IAudioClient,
        render: IAudioRenderClient,
        /// Read back from the device: the writer's fill target.
        buffer_frames: u32,
        channels: u16,
        rate: u32,
        sample: Sample,
        format: &'static str,
    }

    /// The id is the endpoint id string, stable across reboots and renames.
    pub fn devices() -> Vec<Device> {
        // On a thread of our own, not whichever apartment the caller is in.
        std::thread::Builder::new()
            .name("wasapi-devices".into())
            .spawn(enumerate)
            .ok()
            .and_then(|thread| thread.join().ok())
            .unwrap_or_default()
    }

    fn enumerate() -> Vec<Device> {
        let Ok(_com) = Com::new() else {
            return Vec::new();
        };
        unsafe {
            let Ok(enumerator) =
                CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL)
            else {
                return Vec::new();
            };
            let Ok(collection) = enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) else {
                return Vec::new();
            };
            let count = collection.GetCount().unwrap_or(0);
            let mut out = Vec::new();
            for index in 0..count {
                let Ok(device) = collection.Item(index) else {
                    continue;
                };
                let Some(id) = endpoint_id(&device) else {
                    continue;
                };
                let name = friendly_name(&device).unwrap_or_else(|| id.clone());
                out.push(Device { id, name });
            }
            out
        }
    }

    /// Errors here become a fallback to shared, so they say what failed.
    pub fn open(request: &Request, shared: &Arc<Shared>) -> Result<OpenOutput, String> {
        let stop = Arc::new(AtomicBool::new(false));
        // Two channels: the thread negotiates before the rings can be sized, and
        // the rings must exist before it writes.
        let (ready_tx, ready_rx) = mpsc::channel::<Result<Ready, String>>();
        let (feed_tx, feed_rx) = mpsc::channel::<Feed>();

        let want = request.clone();
        let thread_shared = shared.clone();
        let thread_stop = stop.clone();
        let writer = std::thread::Builder::new()
            .name("wasapi-out".into())
            .spawn(move || claim(&want, thread_shared, thread_stop, ready_tx, feed_rx))
            .map_err(|e| format!("spawn wasapi writer: {e}"))?;

        let ready = match ready_rx.recv() {
            Ok(Ok(ready)) => ready,
            Ok(Err(e)) => {
                let _ = writer.join();
                return Err(e);
            }
            Err(_) => {
                let _ = writer.join();
                return Err("the wasapi writer stopped before it reported a device".into());
            }
        };

        let (ring_frames, producer, ring, tap_tx, tap) = rings(ready.rate);
        if feed_tx.send(Feed { ring, tap: tap_tx }).is_err() {
            let _ = writer.join();
            return Err("the wasapi writer stopped before it took the ring".into());
        }

        Ok(OpenOutput {
            stream: Box::new(Claim {
                stop,
                writer: Some(writer),
            }),
            negotiated: ready.negotiated,
            sample_rate: ready.rate,
            ring_frames,
            producer,
            tap,
        })
    }

    fn claim(
        request: &Request,
        shared: Arc<Shared>,
        stop: Arc<AtomicBool>,
        ready: Sender<Result<Ready, String>>,
        feed: Receiver<Feed>,
    ) {
        // Declared first so it drops last: CoUninitialize with a live interface
        // would leave the endpoint claimed.
        let _com = match Com::new() {
            Ok(com) => com,
            Err(e) => {
                let _ = ready.send(Err(e));
                return;
            }
        };

        let (claimed, device_name) = match unsafe { negotiate(request) } {
            Ok(claimed) => claimed,
            Err(e) => {
                let _ = ready.send(Err(e));
                return;
            }
        };

        let report = Ready {
            negotiated: Negotiated {
                mode: Mode::Exclusive,
                device: device_name,
                sample_rate: claimed.rate,
                channels: claimed.channels,
                format: claimed.format.into(),
                fallback: None,
            },
            rate: claimed.rate,
        };
        if ready.send(Ok(report)).is_err() {
            return;
        }

        // A send error means `open` gave up; drop the claim.
        let Ok(feed) = feed.recv() else {
            return;
        };

        unsafe {
            match claimed.sample {
                Sample::F32 => run::<f32>(claimed, shared, feed, stop),
                Sample::I32 => run::<i32>(claimed, shared, feed, stop),
                Sample::I16 => run::<i16>(claimed, shared, feed, stop),
            }
        }
    }

    /// Returns the claim and the device's friendly name.
    unsafe fn negotiate(request: &Request) -> Result<(Claimed, String), String> {
        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                    .map_err(|e| format!("opening the device enumerator: {e}"))?;

            // A named endpoint that's gone takes the default, like the shared backend.
            let device = request
                .device
                .as_deref()
                .and_then(|want| {
                    let wide: Vec<u16> = want.encode_utf16().chain(std::iter::once(0)).collect();
                    enumerator.GetDevice(PCWSTR(wide.as_ptr())).ok()
                })
                .map(Ok)
                .unwrap_or_else(|| {
                    enumerator
                        .GetDefaultAudioEndpoint(eRender, eConsole)
                        .map_err(|e| format!("no default render endpoint: {e}"))
                })?;
            let name = friendly_name(&device).unwrap_or_else(|| "unknown endpoint".into());

            let client: IAudioClient = device
                .Activate(CLSCTX_ALL, None)
                .map_err(|e| format!("activating {name}: {e}"))?;

            let (mut default_hns, mut min_hns) = (0i64, 0i64);
            client
                .GetDevicePeriod(Some(&mut default_hns), Some(&mut min_hns))
                .map_err(|e| format!("device period: {e}"))?;

            // Exactly this rate or the claim fails and the seam says why; no silent
            // conversion behind a bit-perfect toggle.
            let rate = request.rate.unwrap_or(DEFAULT_RATE);

            // Stereo first, then the mix format's own count for surround-only endpoints.
            let mut layouts = vec![2u16];
            if let Ok(mix) = client.GetMixFormat()
                && !mix.is_null()
            {
                let channels = (*mix).nChannels;
                if channels != 2 && channels != 0 {
                    layouts.push(channels);
                }
                CoTaskMemFree(Some(mix.cast()));
            }

            let want_format = request.format.as_deref();
            let picked = layouts.iter().find_map(|&channels| {
                pick_format(want_format, |candidate| {
                    supports(&client, candidate, rate, channels)
                })
                .map(|candidate| (candidate, channels))
            });
            let Some((candidate, channels)) = picked else {
                return Err(format!(
                    "{name} takes no format rox can write at {rate} Hz in exclusive mode"
                ));
            };

            let period = period_hns(request.period_ms, default_hns, min_hns);
            let client = initialize(
                &device,
                client,
                candidate,
                rate,
                channels,
                buffer_hns(period),
                period,
            )
            .map_err(|e| format!("claiming {name} exclusively: {e}"))?;

            let buffer_frames = client
                .GetBufferSize()
                .map_err(|e| format!("buffer size: {e}"))?;
            if buffer_frames == 0 {
                return Err(format!("{name} reported an empty buffer"));
            }
            let render: IAudioRenderClient = client
                .GetService()
                .map_err(|e| format!("render client: {e}"))?;

            Ok((
                Claimed {
                    client,
                    render,
                    buffer_frames,
                    channels,
                    rate,
                    sample: candidate.sample,
                    format: candidate.name,
                },
                name,
            ))
        }
    }

    /// `Initialize` plus the alignment retry: after
    /// `AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED`, `GetBufferSize` gives the wanted count,
    /// and a failed client can't be initialized again, so a fresh one is activated.
    unsafe fn initialize(
        device: &IMMDevice,
        client: IAudioClient,
        candidate: &Candidate,
        rate: u32,
        channels: u16,
        buffer: i64,
        period: i64,
    ) -> Result<IAudioClient, Error> {
        unsafe {
            let format = wave_format(candidate, rate, channels);
            // Buffer several periods deep, periodicity one period: legal in push mode.
            match client.Initialize(
                AUDCLNT_SHAREMODE_EXCLUSIVE,
                0,
                buffer,
                period,
                format_ptr(&format),
                None,
            ) {
                Ok(()) => Ok(client),
                Err(e) if e.code() == AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED => {
                    let frames = client.GetBufferSize()?;
                    // Re-ask for the whole aligned buffer; the period only shrinks if it would
                    // no longer fit.
                    let aligned = aligned_hns(frames, rate);
                    drop(client);
                    let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
                    client.Initialize(
                        AUDCLNT_SHAREMODE_EXCLUSIVE,
                        0,
                        aligned,
                        period.min(aligned),
                        format_ptr(&format),
                        None,
                    )?;
                    Ok(client)
                }
                Err(e) => Err(e),
            }
        }
    }

    /// Exclusive mode answers yes or no; there's no closest match.
    unsafe fn supports(
        client: &IAudioClient,
        candidate: &Candidate,
        rate: u32,
        channels: u16,
    ) -> bool {
        unsafe {
            let format = wave_format(candidate, rate, channels);
            // S_OK exactly: S_FALSE offers a near miss, which Initialize would then reject.
            client.IsFormatSupported(AUDCLNT_SHAREMODE_EXCLUSIVE, format_ptr(&format), None) == S_OK
        }
    }

    /// The struct is packed; the whole struct's address is the `Format` field's,
    /// aligned.
    fn format_ptr(format: &WAVEFORMATEXTENSIBLE) -> *const WAVEFORMATEX {
        (format as *const WAVEFORMATEXTENSIBLE).cast()
    }

    /// Always extensible: valid bits and the channel mask are what exclusive
    /// drivers check.
    fn wave_format(candidate: &Candidate, rate: u32, channels: u16) -> WAVEFORMATEXTENSIBLE {
        let block_align = channels * candidate.bits / 8;
        WAVEFORMATEXTENSIBLE {
            Format: WAVEFORMATEX {
                wFormatTag: WAVE_FORMAT_EXTENSIBLE as u16,
                nChannels: channels,
                nSamplesPerSec: rate,
                nAvgBytesPerSec: rate * block_align as u32,
                nBlockAlign: block_align,
                wBitsPerSample: candidate.bits,
                // Bytes past WAVEFORMATEX: valid bits, channel mask, subformat guid.
                cbSize: 22,
            },
            Samples: WAVEFORMATEXTENSIBLE_0 {
                wValidBitsPerSample: candidate.valid_bits,
            },
            dwChannelMask: channel_mask(channels),
            SubFormat: if candidate.float {
                KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
            } else {
                KSDATAFORMAT_SUBTYPE_PCM
            },
        }
    }

    /// The canonical mask per channel count: exclusive drivers refuse a layout
    /// they don't publish, and a low-bit fill spells quad as FL/FR/C/LFE, which
    /// no quad card has. Odd counts keep the low-bit fill.
    fn channel_mask(channels: u16) -> u32 {
        match channels {
            0 | 1 => SPEAKER_FRONT_CENTER,
            2 => SPEAKER_FRONT_LEFT | SPEAKER_FRONT_RIGHT,
            // KSAUDIO_SPEAKER_QUAD, 0x33.
            4 => SPEAKER_FRONT_LEFT | SPEAKER_FRONT_RIGHT | SPEAKER_BACK_LEFT | SPEAKER_BACK_RIGHT,
            // KSAUDIO_SPEAKER_5POINT1, 0x3f.
            6 => {
                SPEAKER_FRONT_LEFT
                    | SPEAKER_FRONT_RIGHT
                    | SPEAKER_FRONT_CENTER
                    | SPEAKER_LOW_FREQUENCY
                    | SPEAKER_BACK_LEFT
                    | SPEAKER_BACK_RIGHT
            }
            // KSAUDIO_SPEAKER_7POINT1_SURROUND, 0x63f: the side pair, as Windows has
            // spelled 7.1 since Vista.
            8 => {
                SPEAKER_FRONT_LEFT
                    | SPEAKER_FRONT_RIGHT
                    | SPEAKER_FRONT_CENTER
                    | SPEAKER_LOW_FREQUENCY
                    | SPEAKER_BACK_LEFT
                    | SPEAKER_BACK_RIGHT
                    | SPEAKER_SIDE_LEFT
                    | SPEAKER_SIDE_RIGHT
            }
            n => u32::MAX >> (32 - n.min(32)),
        }
    }

    /// Stable across renames. COM allocated it, so free it here.
    unsafe fn endpoint_id(device: &IMMDevice) -> Option<String> {
        unsafe {
            let id = device.GetId().ok()?;
            if id.is_null() {
                return None;
            }
            let out = id.to_string().ok();
            CoTaskMemFree(Some(id.as_ptr().cast()));
            out
        }
    }

    /// Non-strings read as absent; the caller falls back to the id.
    unsafe fn friendly_name(device: &IMMDevice) -> Option<String> {
        unsafe {
            let store = device.OpenPropertyStore(STGM_READ).ok()?;
            let mut value = store.GetValue(&PKEY_Device_FriendlyName).ok()?;
            let name = if value.Anonymous.Anonymous.vt == VT_LPWSTR {
                value.Anonymous.Anonymous.Anonymous.pwszVal.to_string().ok()
            } else {
                None
            };
            let _ = PropVariantClear(&mut value);
            name
        }
    }

    /// The staging buffer is allocated once, so the loop allocates nothing,
    /// locks nothing, and does no I/O beyond its two COM calls.
    unsafe fn run<T>(claimed: Claimed, shared: Arc<Shared>, feed: Feed, stop: Arc<AtomicBool>)
    where
        T: SizedSample + FromSample<f32>,
    {
        unsafe {
            let Feed { mut ring, mut tap } = feed;
            let mut staging = vec![
                T::from_sample(0.0f32);
                claimed.buffer_frames as usize * claimed.channels as usize
            ];

            // Pre-roll: exclusive mode plays the buffer the moment Start returns, and an
            // empty one clicks.
            if let Err(e) = top_up(&claimed, &mut staging, &shared, &mut ring, &mut tap) {
                return lost(&shared, format!("wasapi pre-roll: {e}"));
            }
            if let Err(e) = claimed.client.Start() {
                return lost(&shared, format!("wasapi start: {e}"));
            }

            // Two wakes per period against a buffer several periods deep, so a missed
            // wake still leaves frames.
            let interval = Duration::from_secs_f64(
                wake_frames(claimed.buffer_frames) as f64 / claimed.rate as f64,
            )
            .max(Duration::from_millis(1));
            let ticker = Ticker::new(interval);

            let mut faults = 0;
            while !stop.load(Ordering::Acquire) {
                ticker.wait();
                match top_up(&claimed, &mut staging, &shared, &mut ring, &mut tap) {
                    Ok(()) => faults = 0,
                    Err(e) => {
                        // Retry on the next wake; failures before the fill leave the ring
                        // untouched. Fatal errors or a run of faults hand off to the app's reopen.
                        faults += 1;
                        if fatal(&e) || faults > MAX_FAULTS {
                            let _ = claimed.client.Stop();
                            return lost(&shared, format!("wasapi write: {e}"));
                        }
                    }
                }
            }

            // Stop before the client drops so a stale buffer doesn't play out. No
            // drain: this is always a teardown.
            let _ = claimed.client.Stop();
        }
    }

    /// `GetCurrentPadding` is what's still queued; the rest is ours to fill.
    unsafe fn top_up<T>(
        claimed: &Claimed,
        staging: &mut [T],
        shared: &Shared,
        ring: &mut Consumer<f32>,
        tap: &mut Producer<f32>,
    ) -> Result<(), Error>
    where
        T: SizedSample + FromSample<f32>,
    {
        unsafe {
            let padding = claimed.client.GetCurrentPadding()?;
            let free = claimed.buffer_frames.saturating_sub(padding);
            if free == 0 {
                return Ok(());
            }
            let samples = free as usize * claimed.channels as usize;
            // Before the fill, so a refusal leaves the ring and clock untouched.
            let buffer = claimed.render.GetBuffer(free)?;
            fill(
                &mut staging[..samples],
                claimed.channels as usize,
                shared,
                ring,
                tap,
            );
            // Through staging: nothing promises the driver's pointer is aligned for the
            // sample type, and an unaligned write is undefined.
            std::ptr::copy_nonoverlapping(
                staging.as_ptr().cast::<u8>(),
                buffer,
                samples * std::mem::size_of::<T>(),
            );
            claimed.render.ReleaseBuffer(free, 0)
        }
    }

    /// Not worth retrying. Out-of-order means a buffer never went back, which no
    /// retry fixes.
    fn fatal(error: &Error) -> bool {
        const FATAL: &[HRESULT] = &[
            AUDCLNT_E_DEVICE_INVALIDATED,
            AUDCLNT_E_RESOURCES_INVALIDATED,
            AUDCLNT_E_SERVICE_NOT_RUNNING,
            AUDCLNT_E_NOT_INITIALIZED,
            AUDCLNT_E_OUT_OF_ORDER,
        ];
        FATAL.contains(&error.code())
    }

    /// Picked up by the app's reopen path. Logging is fine: the thread is ending.
    fn lost(shared: &Shared, message: String) {
        log::error!("exclusive output: {message}");
        shared.device_lost.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(_: &Candidate) -> bool {
        true
    }

    #[test]
    fn the_ladder_prefers_float_when_everything_fits() {
        let picked = pick_format(None, all).expect("a format");
        assert_eq!(picked.name, "f32");
        assert_eq!(picked.sample, Sample::F32);
    }

    #[test]
    fn a_named_format_the_device_takes_wins() {
        let picked = pick_format(Some("s16"), all).expect("a format");
        assert_eq!(picked.name, "s16");
        assert_eq!(picked.bits, 16);
    }

    #[test]
    fn a_named_format_the_device_refuses_falls_to_the_best_it_has() {
        // The endpoint is 24-in-32 only, and the settings page asked for f32.
        let picked = pick_format(Some("f32"), |c| c.name == "s32").expect("a format");
        assert_eq!(picked.name, "s32");
        assert_eq!((picked.bits, picked.valid_bits), (32, 24));
        assert!(!picked.float);
    }

    #[test]
    fn a_name_no_rung_carries_is_ignored_rather_than_fatal() {
        let picked = pick_format(Some("s24_3le"), all).expect("a format");
        assert_eq!(picked.name, "f32");
    }

    #[test]
    fn a_device_that_takes_nothing_picks_nothing() {
        assert!(pick_format(None, |_| false).is_none());
    }

    #[test]
    fn the_period_takes_the_device_default_when_nothing_is_asked_for() {
        assert_eq!(period_hns(None, 100_000, 30_000), 100_000);
    }

    #[test]
    fn a_requested_period_lands_in_reference_time() {
        assert_eq!(period_hns(Some(5.0), 100_000, 30_000), 50_000);
    }

    #[test]
    fn a_period_under_the_device_minimum_climbs_to_it() {
        assert_eq!(period_hns(Some(1.0), 100_000, 30_000), 30_000);
    }

    #[test]
    fn a_period_deeper_than_the_ring_is_capped() {
        assert_eq!(period_hns(Some(5_000.0), 100_000, 30_000), MAX_PERIOD_HNS);
    }

    #[test]
    fn a_device_minimum_past_the_cap_still_wins() {
        // The driver floor has to survive the cap.
        let huge = MAX_PERIOD_HNS * 2;
        assert_eq!(period_hns(Some(5.0), huge, huge), huge);
    }

    #[test]
    fn the_buffer_holds_several_periods() {
        assert_eq!(buffer_hns(100_000), 400_000);
        assert_eq!(buffer_hns(MAX_PERIOD_HNS), 4_000_000);
    }

    #[test]
    fn the_writer_wakes_twice_a_period() {
        assert_eq!(wake_frames(1920), 240);
        // Zero would be a spin.
        assert_eq!(wake_frames(4), 1);
        assert_eq!(wake_frames(0), 1);
    }

    #[test]
    fn aligned_duration_round_trips_a_whole_number_of_frames() {
        assert_eq!(aligned_hns(480, 48000), 100_000);
    }

    #[test]
    fn aligned_duration_rounds_up_rather_than_landing_short() {
        assert_eq!(aligned_hns(441, 44100), 100_000);
        assert_eq!(aligned_hns(448, 44100), 101_587);
    }
}
