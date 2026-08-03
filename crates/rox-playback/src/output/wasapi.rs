//! Exclusive output on Windows: WASAPI opened in
//! `AUDCLNT_SHAREMODE_EXCLUSIVE`, which hands the endpoint to rox alone and
//! takes the sample rate and format the file actually has instead of the one
//! the audio engine already mixes at. That's the whole point of the mode:
//! shared WASAPI resamples everything to the endpoint's format, so a 96 kHz
//! file never reaches the converter as 96 kHz. cpal has no exclusive path,
//! which is why this talks to the API directly.
//!
//! The shape mirrors `alsa.rs` deliberately: a device list, an `open` that
//! either claims or hands back a reason, a `Claim` whose Drop joins the
//! writer, and a writer that calls the shared `fill` per period. What
//! differs is forced by COM. Interfaces are apartment-bound and not `Send`,
//! so everything from the enumerator to the render client is created, used,
//! and released on one thread we own. `open` starts that thread first and
//! waits for it to report what it negotiated, rather than negotiating on the
//! caller's thread and shipping the handles across.
//!
//! The negotiation math is plain Rust and sits above the FFI, so it compiles
//! and gets tested on every platform, not only where the backend runs.

#[cfg(target_os = "windows")]
pub use platform::{devices, open};

/// Reference time, the unit every WASAPI duration is in: 100 ns ticks, so ten
/// million to the second.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const HNS_PER_SEC: f64 = 10_000_000.0;

/// The rate to ask for when the caller doesn't name one, which is every
/// session that opens before a file has been decoded. The pump reopens at the
/// file's own rate once it knows it.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const DEFAULT_RATE: u32 = 48000;

/// The longest period we'll ask a device for, 100 ms. Two things push down on
/// this. The buffer is [`PERIODS`] of them and the sample ring in front of the
/// writer holds 500 ms, so a longer period asks for a device buffer the decode
/// thread can't keep stocked. And the writer only notices the stop flag once a
/// wake, so a huge period turns toggling exclusive off into a visible stall.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const MAX_PERIOD_HNS: i64 = 1_000_000;

/// Which Rust sample type a negotiated format writes as. The writer thread is
/// generic over it, so this picks the instantiation.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
enum Sample {
    F32,
    I32,
    I16,
}

/// One rung of the format ladder: what goes into the `WAVEFORMATEXTENSIBLE`
/// and what rox writes into the buffer.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
struct Candidate {
    sample: Sample,
    /// The container width, `wBitsPerSample`.
    bits: u16,
    /// How many of those bits the device actually uses, `wValidBitsPerSample`.
    valid_bits: u16,
    /// True for `KSDATAFORMAT_SUBTYPE_IEEE_FLOAT`, false for the PCM subtype.
    float: bool,
    /// The name the settings page pins and [`Negotiated`] reports, spelled the
    /// same way the ALSA backend spells it so one setting means one thing on
    /// both platforms.
    ///
    /// [`Negotiated`]: super::Negotiated
    name: &'static str,
}

/// Formats we'll take, best first. Float straight through where the endpoint
/// has it, then 24 bits carried in a 32-bit container, then 16. The middle
/// rung is what "24-bit" means on Windows: drivers advertise a 32-bit
/// container with 24 valid bits, and rox writes a plain `i32` whose top 24
/// bits are the sample, so it's reported as `s32` rather than inventing a
/// name for a layout that has no Rust type of its own. Packed 24-bit is
/// deliberately absent for the same reason it is in the ALSA backend.
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

/// Walk the ladder against whatever the device says yes to. A named format
/// the device takes wins; anything else, including a name for a format this
/// device doesn't have, falls to the best it does. Reporting the result is
/// what keeps that honest, so a pick the hardware refused reads as the format
/// actually running.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn pick_format(
    want: Option<&str>,
    mut supported: impl FnMut(&Candidate) -> bool,
) -> Option<&'static Candidate> {
    if let Some(named) = want.and_then(|want| FORMATS.iter().find(|c| c.name == want)) {
        if supported(named) {
            return Some(named);
        }
    }
    FORMATS.iter().find(|c| supported(c))
}

/// Periods in the endpoint buffer. Only event-driven exclusive mode forces the
/// buffer to be exactly one period; a push-mode client like this one asks for
/// a deeper buffer and leaves the periodicity at a single period. Four is what
/// the ALSA backend takes and it's the same reasoning: the writer wakes on a
/// timer, and a buffer one period deep is dry the moment a wake lands late.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const PERIODS: i64 = 4;

/// The device period to ask for, in reference time. `default_hns` and
/// `min_hns` come from `GetDevicePeriod`, and the driver's own minimum is a
/// hard floor because asking below it fails the claim outright.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn period_hns(want_ms: Option<f64>, default_hns: i64, min_hns: i64) -> i64 {
    let want = want_ms
        .map(|ms| (ms / 1000.0 * HNS_PER_SEC).round() as i64)
        .unwrap_or(default_hns);
    want.clamp(min_hns.max(1), MAX_PERIOD_HNS.max(min_hns.max(1)))
}

/// The buffer duration that goes with a period: [`PERIODS`] of them. The
/// capped period puts the ceiling at 400 ms, the same depth ALSA's four
/// periods reach, which the 500 ms ring in front can still keep stocked.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn buffer_hns(period: i64) -> i64 {
    period.saturating_mul(PERIODS)
}

/// How long the writer sleeps between top-ups, in frames: half a period.
/// Taken off the buffer the driver actually handed back rather than the
/// duration we asked for, so an alignment retry or a driver clamp is followed
/// instead of guessed at.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn wake_frames(buffer_frames: u32) -> u32 {
    (buffer_frames / (PERIODS as u32 * 2)).max(1)
}

/// The other half of the alignment dance: a driver that rejects a duration
/// tells us the frame count it wanted through `GetBufferSize`, and this turns
/// that count back into the duration to re-initialize with. The half-tick is
/// the rounding Microsoft's own sample does, and it matters: rounding down
/// lands one frame short and the driver refuses again.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn aligned_hns(frames: u32, rate: u32) -> i64 {
    (HNS_PER_SEC / rate as f64 * frames as f64 + 0.5) as i64
}

#[cfg(target_os = "windows")]
mod platform {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{self, Receiver, Sender};
    use std::sync::Arc;
    use std::thread::JoinHandle;
    use std::time::Duration;

    use cpal::{FromSample, SizedSample};
    use rtrb::{Consumer, Producer};
    use windows::core::{Error, HRESULT, PCWSTR};
    use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
    use windows::Win32::Foundation::{CloseHandle, HANDLE, S_OK};
    use windows::Win32::Media::Audio::{
        eConsole, eRender, IAudioClient, IAudioRenderClient, IMMDevice, IMMDeviceEnumerator,
        MMDeviceEnumerator, AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED, AUDCLNT_E_DEVICE_INVALIDATED,
        AUDCLNT_E_NOT_INITIALIZED, AUDCLNT_E_OUT_OF_ORDER, AUDCLNT_E_RESOURCES_INVALIDATED,
        AUDCLNT_E_SERVICE_NOT_RUNNING, AUDCLNT_SHAREMODE_EXCLUSIVE, DEVICE_STATE_ACTIVE,
        WAVEFORMATEX, WAVEFORMATEXTENSIBLE, WAVEFORMATEXTENSIBLE_0,
    };
    use windows::Win32::Media::KernelStreaming::{
        KSDATAFORMAT_SUBTYPE_PCM, SPEAKER_BACK_LEFT, SPEAKER_BACK_RIGHT, SPEAKER_FRONT_CENTER,
        SPEAKER_FRONT_LEFT, SPEAKER_FRONT_RIGHT, SPEAKER_LOW_FREQUENCY, SPEAKER_SIDE_LEFT,
        SPEAKER_SIDE_RIGHT, WAVE_FORMAT_EXTENSIBLE,
    };
    use windows::Win32::Media::Multimedia::KSDATAFORMAT_SUBTYPE_IEEE_FLOAT;
    use windows::Win32::System::Com::StructuredStorage::PropVariantClear;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
        COINIT_MULTITHREADED, STGM_READ,
    };
    use windows::Win32::System::Threading::{
        CreateWaitableTimerExW, SetWaitableTimer, WaitForSingleObject,
        CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, INFINITE, TIMER_ALL_ACCESS,
    };
    use windows::Win32::System::Variant::VT_LPWSTR;

    use super::super::{fill, rings, Device, Mode, Negotiated, OpenOutput, OutputStream, Request};
    use super::{
        aligned_hns, buffer_hns, period_hns, pick_format, wake_frames, Candidate, Sample,
        DEFAULT_RATE,
    };
    use crate::shared::Shared;

    /// How many write faults in a row before the device counts as gone. An
    /// endpoint can hiccup once and carry on; one that won't take a buffer
    /// after a few tries isn't coming back, and the app's reopen path takes
    /// it from there.
    const MAX_FAULTS: u32 = 4;

    /// COM initialized for as long as this lives. Every caller here owns the
    /// thread it runs on, so nothing has put that thread in an apartment yet
    /// and the multithreaded one always takes. Doing it this way rather than
    /// borrowing the caller's apartment is the point: rox's UI thread is an
    /// STA on Windows, and an interface created there could not legally be
    /// touched from the writer thread.
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

    /// A periodic wake for the writer thread. `thread::sleep` won't do: the
    /// Windows timer tick is about 15.6 ms unless something in the process
    /// raised it, so a sleep asked for half of a 10 ms period comes back
    /// three periods late. A high-resolution waitable timer (Windows 10 1803
    /// and up) fires on time. Where one can't be created we fall back to
    /// sleeping, and the spare periods in the buffer are what keep that from
    /// being a dropout on every wake.
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
                // A negative due time is relative and in reference time; the
                // period is milliseconds and has to be at least 1, or the
                // timer fires once and never again.
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
                // Nothing to check on the way out: the timer is periodic, so
                // the only ways this returns are a tick and an abandon, and
                // both want the same next move, which is to look at the
                // endpoint's padding.
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

    /// The claim on the endpoint: the writer thread plus its stop flag.
    /// Dropping it stops audio and hands the device back, which is what makes
    /// toggling exclusive off actually release it.
    struct Claim {
        stop: Arc<AtomicBool>,
        writer: Option<JoinHandle<()>>,
    }

    impl OutputStream for Claim {}

    impl Drop for Claim {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            // The thread checks the flag once per wake and blocks on the
            // timer in between, so this waits half a period at worst.
            // Joining rather than detaching matters: every COM interface
            // lives in that thread, and a detached thread would still hold
            // the endpoint while the next session tried to claim it.
            if let Some(writer) = self.writer.take() {
                let _ = writer.join();
            }
        }
    }

    /// What the writer thread reports back once the device has agreed to
    /// something, so `open` can size the rings and answer its caller.
    struct Ready {
        negotiated: Negotiated,
        rate: u32,
    }

    /// The rings, handed to the writer thread after `open` has allocated them
    /// at the negotiated rate.
    struct Feed {
        ring: Consumer<f32>,
        tap: Producer<f32>,
    }

    /// Everything the writer thread holds on the device side. All of it is
    /// COM and none of it leaves the thread.
    struct Claimed {
        client: IAudioClient,
        render: IAudioRenderClient,
        /// Frames the endpoint buffer holds, read back from the device rather
        /// than remembered: it's the writer's fill target, and guessing it
        /// wrong means writing past what the driver asked for.
        buffer_frames: u32,
        channels: u16,
        rate: u32,
        sample: Sample,
        /// The ladder rung's name, carried rather than looked back up, so
        /// what the UI reads is the rung the device actually took.
        format: &'static str,
    }

    /// Every active render endpoint, by the name the sound control panel
    /// shows. The id is the endpoint id string, which survives a reboot and a
    /// rename, unlike the friendly name.
    pub fn devices() -> Vec<Device> {
        // COM is thread-bound, so the enumeration runs on a thread of our own
        // instead of initializing an apartment on whichever thread the
        // settings page happened to call from.
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

    /// Claim a device and start the writer thread. Every error here is one
    /// the seam turns into a fallback to shared output, so they say what
    /// failed rather than just that something did.
    pub fn open(request: &Request, shared: &Arc<Shared>) -> Result<OpenOutput, String> {
        let stop = Arc::new(AtomicBool::new(false));
        // Two channels because the handshake goes both ways: the thread has
        // to negotiate before anyone knows the rate to size the rings at, and
        // the rings have to exist before the thread can write a frame.
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

    /// The writer thread from end to end: initialize COM, claim and settle
    /// the device, report it, take the rings, then run until stopped.
    fn claim(
        request: &Request,
        shared: Arc<Shared>,
        stop: Arc<AtomicBool>,
        ready: Sender<Result<Ready, String>>,
        feed: Receiver<Feed>,
    ) {
        // Declared first so it outlives every interface below; locals drop in
        // reverse, and calling CoUninitialize with a live interface still
        // held would leave the endpoint claimed.
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

        // `open` allocates the rings once it knows the rate. A send error
        // here means it gave up in between, so drop the claim and go.
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

    /// Pick the endpoint, settle a format on it, and initialize the client.
    /// Returns the claim and the device's friendly name.
    unsafe fn negotiate(request: &Request) -> Result<(Claimed, String), String> {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .map_err(|e| format!("opening the device enumerator: {e}"))?;

        // A named endpoint that's gone (unplugged, disabled, a driver
        // reinstall that changed the id) takes the default one rather than
        // failing the open, matching what the shared backend does with a
        // stale cpal name.
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

        // Exclusive mode means exactly this rate or nothing. There's no
        // nearest-match to fall to the way ALSA has: the endpoint either
        // opens at the file's rate or the claim fails and the seam reports
        // why, which beats silently converting behind a bit-perfect toggle.
        let rate = request.rate.unwrap_or(DEFAULT_RATE);

        // Stereo first because that's what the ring carries; the mix format's
        // own count is the fallback for an endpoint that only offers its
        // surround layout, and `fill` folds onto whatever comes back the same
        // way it does for cpal.
        let mut layouts = vec![2u16];
        if let Ok(mix) = client.GetMixFormat() {
            if !mix.is_null() {
                let channels = (*mix).nChannels;
                if channels != 2 && channels != 0 {
                    layouts.push(channels);
                }
                CoTaskMemFree(Some(mix.cast()));
            }
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

    /// `IAudioClient::Initialize` plus the alignment dance. A driver whose
    /// buffer has to land on a frame boundary rejects the first duration with
    /// `AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED` and then tells us, through
    /// `GetBufferSize`, the count it wanted. The documented recovery is to
    /// throw the client away and activate a fresh one: an `IAudioClient` that
    /// failed Initialize can't be initialized again.
    unsafe fn initialize(
        device: &IMMDevice,
        client: IAudioClient,
        candidate: &Candidate,
        rate: u32,
        channels: u16,
        buffer: i64,
        period: i64,
    ) -> Result<IAudioClient, Error> {
        let format = wave_format(candidate, rate, channels);
        // The two durations differ on purpose: the buffer holds several
        // periods so a late wake still has frames to play, while the
        // periodicity stays at the one period the device schedules on. That's
        // legal for a push-mode client; only the event-driven flag forces the
        // two to be equal.
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
                // The frame count the driver hands back is the whole buffer,
                // so that's the duration that gets re-asked for. The period
                // rides along unchanged unless the aligned buffer came back
                // shorter than it, which would be a periodicity the buffer
                // can't hold.
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

    /// Ask the endpoint whether it takes one rung of the ladder. Exclusive
    /// mode answers yes or no and nothing else: the closest-match out
    /// parameter is only filled in for shared mode, so there's nothing to ask
    /// for here.
    unsafe fn supports(
        client: &IAudioClient,
        candidate: &Candidate,
        rate: u32,
        channels: u16,
    ) -> bool {
        let format = wave_format(candidate, rate, channels);
        // S_OK exactly, not "not an error": S_FALSE is the shared-mode answer
        // meaning a near miss is on offer, and taking that as a yes here is
        // how a format the endpoint never agreed to ends up in Initialize.
        client.IsFormatSupported(AUDCLNT_SHAREMODE_EXCLUSIVE, format_ptr(&format), None) == S_OK
    }

    /// `WAVEFORMATEXTENSIBLE` is packed, so a reference to its `Format` field
    /// would be an unaligned borrow. The extensible struct starts with that
    /// field, so casting the whole struct's address is the same pointer and
    /// is always aligned.
    fn format_ptr(format: &WAVEFORMATEXTENSIBLE) -> *const WAVEFORMATEX {
        (format as *const WAVEFORMATEXTENSIBLE).cast()
    }

    /// Spell one ladder rung as a format the driver can answer about. Always
    /// extensible rather than plain `WAVEFORMATEX`: valid-bits and the
    /// channel mask are the two things an exclusive-mode driver checks, and
    /// only the extensible form carries them.
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
                // The bytes past WAVEFORMATEX: valid bits, channel mask,
                // subformat guid.
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

    /// The speaker layout a format claims. Every count a consumer endpoint
    /// actually offers gets the canonical mask the driver checks for, because
    /// an exclusive-mode driver compares the mask and refuses a layout it
    /// doesn't publish. Filling the low bits instead would spell four channels
    /// as front left, right, centre and LFE, which no quad card has, so the
    /// claim would fail and the mode would quietly fall back to shared. Odd
    /// counts nobody ships keep the low-bit fill.
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
            // KSAUDIO_SPEAKER_7POINT1_SURROUND, 0x63f. The side pair rather
            // than the front-of-centre pair: the old 7.1 layout predates side
            // speakers and Windows has spelled 7.1 this way since Vista.
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

    /// The endpoint id string, the one identifier that survives a rename.
    /// `GetId` hands back memory COM allocated, so it's freed here rather
    /// than leaked once per device per settings visit.
    unsafe fn endpoint_id(device: &IMMDevice) -> Option<String> {
        let id = device.GetId().ok()?;
        if id.is_null() {
            return None;
        }
        let out = id.to_string().ok();
        CoTaskMemFree(Some(id.as_ptr().cast()));
        out
    }

    /// The name a human reads, out of the endpoint's property store. Anything
    /// that isn't a string is treated as absent; the caller falls back to the
    /// id, which is ugly but never blank.
    unsafe fn friendly_name(device: &IMMDevice) -> Option<String> {
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

    /// The writer thread's loop. The staging buffer is allocated once here
    /// and refilled in place, so the loop itself allocates nothing, takes no
    /// lock, and does no I/O beyond the two COM calls it exists for.
    unsafe fn run<T>(claimed: Claimed, shared: Arc<Shared>, feed: Feed, stop: Arc<AtomicBool>)
    where
        T: SizedSample + FromSample<f32>,
    {
        let Feed { mut ring, mut tap } = feed;
        let mut staging = vec![
            T::from_sample(0.0f32);
            claimed.buffer_frames as usize * claimed.channels as usize
        ];

        // Pre-roll before Start, because in exclusive mode the endpoint plays
        // whatever is in the buffer the moment Start returns and an unfilled
        // one is a click. It's the same cushion the ALSA backend buys with a
        // start threshold of a whole buffer.
        if let Err(e) = top_up(&claimed, &mut staging, &shared, &mut ring, &mut tap) {
            return lost(&shared, format!("wasapi pre-roll: {e}"));
        }
        if let Err(e) = claimed.client.Start() {
            return lost(&shared, format!("wasapi start: {e}"));
        }

        // Half a period per wake, against a buffer several periods deep. Two
        // wakes per period is enough to keep it topped up, and the periods
        // behind the one playing are the slack a late wake spends: miss one
        // entirely and the device still has frames. Anything longer than that
        // comes out of the 500 ms ring in front.
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
                    // A glitch the driver can shrug off gets retried on the
                    // next wake rather than dropping the stream. Nothing is
                    // lost by waiting: the failures that can happen before
                    // the fill leave the ring untouched, and the one that
                    // can happen after it is out-of-order, which is fatal
                    // here anyway. A device that won't take a buffer after a
                    // run of tries is gone, and the app's reopen path takes
                    // it from there.
                    faults += 1;
                    if fatal(&e) || faults > MAX_FAULTS {
                        let _ = claimed.client.Stop();
                        return lost(&shared, format!("wasapi write: {e}"));
                    }
                }
            }
        }

        // Stop before the client drops so the endpoint goes idle instead of
        // playing out a stale buffer while the next session tries to claim
        // it. Deliberately not drained first: every path here is a teardown
        // and the caller is blocked in join waiting for it.
        let _ = claimed.client.Stop();
    }

    /// One pass over the endpoint buffer: take what's free, fill it, hand it
    /// back. `GetCurrentPadding` is the frames still queued, so the rest of
    /// the buffer is ours.
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
        let padding = claimed.client.GetCurrentPadding()?;
        let free = claimed.buffer_frames.saturating_sub(padding);
        if free == 0 {
            return Ok(());
        }
        let samples = free as usize * claimed.channels as usize;
        // Asked for before the fill, so a refusal costs nothing: the ring
        // still holds its frames and the clock hasn't moved.
        let buffer = claimed.render.GetBuffer(free)?;
        fill(
            &mut staging[..samples],
            claimed.channels as usize,
            shared,
            ring,
            tap,
        );
        // Through a staging buffer rather than writing the driver's memory
        // directly, because nothing in the contract promises that pointer is
        // aligned for the sample type and an unaligned write would be
        // undefined. One memcpy per period is nothing next to that.
        std::ptr::copy_nonoverlapping(
            staging.as_ptr().cast::<u8>(),
            buffer,
            samples * std::mem::size_of::<T>(),
        );
        claimed.render.ReleaseBuffer(free, 0)
    }

    /// Errors there's no point retrying: the endpoint went away, the session
    /// lost its resources, or the audio service stopped. Out-of-order is in
    /// here too, because it means a buffer we took never went back and no
    /// number of retries releases it.
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

    /// Flag the device as gone the same way cpal's error callback does, so
    /// the player's existing reopen path picks it up. Logging is fine here:
    /// this is the last thing the writer thread does before it stops.
    fn lost(shared: &Shared, message: String) {
        log::error!("exclusive output: {message}");
        shared.device_lost.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ladder as a device that takes everything sees it.
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
        // 10 ms default, 3 ms minimum, the usual shape of a shared endpoint.
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
        // Nonsense hardware, but the floor is the one number that fails the
        // claim outright, so it has to survive the cap.
        let huge = MAX_PERIOD_HNS * 2;
        assert_eq!(period_hns(Some(5.0), huge, huge), huge);
    }

    #[test]
    fn the_buffer_holds_several_periods() {
        // A 10 ms period gets a 40 ms buffer, and the capped period puts the
        // deepest buffer at 400 ms.
        assert_eq!(buffer_hns(100_000), 400_000);
        assert_eq!(buffer_hns(MAX_PERIOD_HNS), 4_000_000);
    }

    #[test]
    fn the_writer_wakes_twice_a_period() {
        // 1920 frames is four 10 ms periods at 48 kHz, so half a period is
        // 240 of them.
        assert_eq!(wake_frames(1920), 240);
        // Whatever the driver hands back, sleeping for no frames at all would
        // be a spin.
        assert_eq!(wake_frames(4), 1);
        assert_eq!(wake_frames(0), 1);
    }

    #[test]
    fn aligned_duration_round_trips_a_whole_number_of_frames() {
        // 480 frames at 48 kHz is exactly 10 ms.
        assert_eq!(aligned_hns(480, 48000), 100_000);
    }

    #[test]
    fn aligned_duration_rounds_up_rather_than_landing_short() {
        // 441 frames at 44.1 kHz is 100000 exactly; 448 is 101587.3, and
        // rounding down there is the frame that makes the driver refuse
        // again.
        assert_eq!(aligned_hns(441, 44100), 100_000);
        assert_eq!(aligned_hns(448, 44100), 101_587);
    }
}
