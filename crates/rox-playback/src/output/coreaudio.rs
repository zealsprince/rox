//! Exclusive output on macOS: the device taken in hog mode, the one CoreAudio
//! route where the nominal rate we set is the rate the converter runs at
//! (ADR 19). A pull model: the HAL calls [`io_proc`] on its own real-time
//! thread, which hands the same [`fill`] the same buffer.
//!
//! SDK constants are declared by hand rather than via `coreaudio-sys`, whose
//! bindgen needs the macOS SDK, so this type-checks from a cross build. Each
//! selector's four-char code is in its doc comment, one `grep` away from
//! `AudioHardware.h`.

#![allow(non_snake_case)]

use std::ffi::{c_char, c_long, c_void};
use std::mem::MaybeUninit;
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use rtrb::{Consumer, Producer};

use super::{Device, Mode, Negotiated, OpenOutput, OutputStream, Request, fill, rings};
use crate::shared::Shared;

/// Before a file is decoded; the pump reopens at the file's rate.
const DEFAULT_RATE: u32 = 48000;

/// Setting the rate is asynchronous. 100 polls of 5 ms covers the half second
/// USB interfaces take to relock, and a built-in output that settles at once
/// costs one step.
const RATE_SETTLE_POLLS: u32 = 100;
const RATE_SETTLE_STEP: Duration = Duration::from_millis(5);

const NOBODY: Pid = -1;

/// Floor for the deinterleaved scratch when the device won't report its buffer.
const SCRATCH_FLOOR: usize = 4096;

// --- CoreAudio types ------------------------------------------------------

type OSStatus = i32;
type AudioObjectID = u32;
type Pid = i32;

/// The HAL hands back a token, not our pointer; we only store and return it.
type AudioDeviceIOProcID = *mut c_void;

/// Never read, so the struct (and its SMPTETime ABI risk) stays out.
type AudioTimeStampRef = *const c_void;

type AudioDeviceIOProc = unsafe extern "C" fn(
    device: AudioObjectID,
    now: AudioTimeStampRef,
    input_data: *const AudioBufferList,
    input_time: AudioTimeStampRef,
    output_data: *mut AudioBufferList,
    output_time: AudioTimeStampRef,
    client_data: *mut c_void,
) -> OSStatus;

type AudioObjectPropertyListenerProc = unsafe extern "C" fn(
    object: AudioObjectID,
    number_addresses: u32,
    addresses: *const AudioObjectPropertyAddress,
    client_data: *mut c_void,
) -> OSStatus;

#[repr(C)]
#[derive(Clone, Copy)]
struct AudioObjectPropertyAddress {
    /// `mSelector`
    selector: u32,
    /// `mScope`
    scope: u32,
    /// `mElement`
    element: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct AudioBuffer {
    /// `mNumberChannels`, the channels interleaved *within this one buffer*.
    number_channels: u32,
    /// `mDataByteSize`
    data_byte_size: u32,
    /// `mData`
    data: *mut c_void,
}

/// C's flexible array: declares one [`AudioBuffer`] but the HAL hands over
/// `number_buffers`. Only ever read through the HAL's pointer.
#[repr(C)]
struct AudioBufferList {
    /// `mNumberBuffers`
    number_buffers: u32,
    /// `mBuffers`
    buffers: [AudioBuffer; 1],
}

/// Every field is declared because [`property`] checks the byte count the HAL
/// wrote; only three are read.
#[repr(C)]
#[derive(Clone, Copy)]
#[allow(dead_code)]
struct StreamFormat {
    /// `mSampleRate`
    sample_rate: f64,
    /// `mFormatID`
    format_id: u32,
    /// `mFormatFlags`
    format_flags: u32,
    /// `mBytesPerPacket`
    bytes_per_packet: u32,
    /// `mFramesPerPacket`
    frames_per_packet: u32,
    /// `mBytesPerFrame`
    bytes_per_frame: u32,
    /// `mChannelsPerFrame`
    channels_per_frame: u32,
    /// `mBitsPerChannel`
    bits_per_channel: u32,
    /// `mReserved`
    reserved: u32,
}

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Debug)]
struct AudioValueRange {
    /// `mMinimum`
    minimum: f64,
    /// `mMaximum`
    maximum: f64,
}

// --- CoreAudio constants --------------------------------------------------

/// `kAudioObjectSystemObject`, the one object that's always there and owns
/// the device list.
const SYSTEM_OBJECT: AudioObjectID = 1;

/// `kAudioObjectPropertyScopeGlobal`, `'glob'`.
const SCOPE_GLOBAL: u32 = 0x676C_6F62;
/// `kAudioObjectPropertyScopeOutput`, `'outp'`.
const SCOPE_OUTPUT: u32 = 0x6F75_7470;
/// `kAudioObjectPropertyElementMain`, 0 (spelled `...ElementMaster` before
/// macOS 12, same value).
const ELEMENT_MAIN: u32 = 0;

/// `kAudioHardwarePropertyDevices`, `'dev#'`. An array of AudioObjectID.
const HARDWARE_DEVICES: u32 = 0x6465_7623;
/// `kAudioHardwarePropertyDefaultOutputDevice`, `'dOut'`. One AudioObjectID.
const HARDWARE_DEFAULT_OUTPUT: u32 = 0x644F_7574;

/// `kAudioObjectPropertyName`, `'lnam'`. A CFStringRef the caller releases.
/// This is the same selector as the deprecated
/// `kAudioDevicePropertyDeviceNameCFString`, which is why only one is here.
const OBJECT_NAME: u32 = 0x6C6E_616D;
/// `kAudioDevicePropertyDeviceUID`, `'uid '` (trailing space). A CFStringRef
/// the caller releases.
const DEVICE_UID: u32 = 0x7569_6420;
/// `kAudioDevicePropertyDeviceIsAlive`, `'livn'`. A UInt32, 0 once the device
/// is gone.
const DEVICE_IS_ALIVE: u32 = 0x6C69_766E;
/// `kAudioDevicePropertyHogMode`, `'oink'`. A `pid_t`.
const DEVICE_HOG_MODE: u32 = 0x6F69_6E6B;
/// `kAudioDevicePropertyNominalSampleRate`, `'nsrt'`. A Float64.
const DEVICE_NOMINAL_SAMPLE_RATE: u32 = 0x6E73_7274;
/// `kAudioDevicePropertyAvailableNominalSampleRates`, `'nsr#'`. An array of
/// AudioValueRange; a device with discrete rates reports each as a range
/// whose ends are equal.
const DEVICE_AVAILABLE_SAMPLE_RATES: u32 = 0x6E73_7223;
/// `kAudioDevicePropertyStreamConfiguration`, `'slay'`. A variable-length
/// AudioBufferList describing the buffers an IOProc will be handed.
const DEVICE_STREAM_CONFIGURATION: u32 = 0x736C_6179;
/// `kAudioDevicePropertyStreams`, `'stm#'`. An array of AudioObjectID, the
/// stream objects a device is made of.
const DEVICE_STREAMS: u32 = 0x7374_6D23;
/// `kAudioStreamPropertyVirtualFormat`, `'sfmt'`. An
/// AudioStreamBasicDescription: the format an IOProc is handed, as opposed to
/// the physical one the HAL puts on the wire.
const STREAM_VIRTUAL_FORMAT: u32 = 0x7366_6D74;
/// `kAudioFormatLinearPCM`, `'lpcm'`.
const FORMAT_LINEAR_PCM: u32 = 0x6C70_636D;
/// `kAudioFormatFlagIsFloat`, bit 0 of `mFormatFlags`.
const FORMAT_FLAG_IS_FLOAT: u32 = 1;
/// `kAudioDevicePropertyBufferFrameSize`, `'fsiz'`. A UInt32 of frames.
const DEVICE_BUFFER_FRAME_SIZE: u32 = 0x6673_697A;
/// `kAudioDevicePropertyBufferFrameSizeRange`, `'fsz#'`. One AudioValueRange
/// in frames.
const DEVICE_BUFFER_FRAME_SIZE_RANGE: u32 = 0x6673_7A23;

/// `kCFStringEncodingUTF8`.
const CFSTRING_ENCODING_UTF8: u32 = 0x0800_0100;

// --- CoreAudio and CoreFoundation entry points ----------------------------

#[link(name = "CoreAudio", kind = "framework")]
unsafe extern "C" {
    fn AudioObjectGetPropertyDataSize(
        object: AudioObjectID,
        address: *const AudioObjectPropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        out_size: *mut u32,
    ) -> OSStatus;

    fn AudioObjectGetPropertyData(
        object: AudioObjectID,
        address: *const AudioObjectPropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        io_size: *mut u32,
        out_data: *mut c_void,
    ) -> OSStatus;

    fn AudioObjectSetPropertyData(
        object: AudioObjectID,
        address: *const AudioObjectPropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: u32,
        data: *const c_void,
    ) -> OSStatus;

    fn AudioObjectAddPropertyListener(
        object: AudioObjectID,
        address: *const AudioObjectPropertyAddress,
        listener: AudioObjectPropertyListenerProc,
        client_data: *mut c_void,
    ) -> OSStatus;

    fn AudioObjectRemovePropertyListener(
        object: AudioObjectID,
        address: *const AudioObjectPropertyAddress,
        listener: AudioObjectPropertyListenerProc,
        client_data: *mut c_void,
    ) -> OSStatus;

    fn AudioDeviceCreateIOProcID(
        device: AudioObjectID,
        proc_: AudioDeviceIOProc,
        client_data: *mut c_void,
        out_proc_id: *mut AudioDeviceIOProcID,
    ) -> OSStatus;

    fn AudioDeviceDestroyIOProcID(device: AudioObjectID, proc_id: AudioDeviceIOProcID) -> OSStatus;

    fn AudioDeviceStart(device: AudioObjectID, proc_id: AudioDeviceIOProcID) -> OSStatus;

    fn AudioDeviceStop(device: AudioObjectID, proc_id: AudioDeviceIOProcID) -> OSStatus;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFStringGetLength(string: *const c_void) -> c_long;
    fn CFStringGetCString(
        string: *const c_void,
        buffer: *mut c_char,
        buffer_size: c_long,
        encoding: u32,
    ) -> u8;
    fn CFRelease(object: *const c_void);
}

unsafe extern "C" {
    /// From libSystem, which every macOS binary links anyway.
    fn getpid() -> Pid;
}

// --- The claim ------------------------------------------------------------

/// What the IOProc reads, boxed once at open behind the HAL's `void*`. The
/// scratch is sized up front so rendering never allocates.
struct State {
    shared: Arc<Shared>,
    ring: Consumer<f32>,
    tap: Producer<f32>,
    /// Staging for devices that hand out one buffer per stream.
    scratch: Vec<f32>,
}

/// Dropping it stops the IOProc, unregisters it, frees what it read, and
/// releases hog mode. Built up as `open` progresses, so a failure halfway
/// unwinds through this same Drop; every field is checked before use.
struct Claim {
    device: AudioObjectID,
    proc_id: AudioDeviceIOProcID,
    started: bool,
    state: *mut State,
    /// Its own allocation, not a borrow of `state`: the listener runs on the
    /// notification thread while the IOProc mutates the rings.
    listener: *mut Arc<Shared>,
    hogged: bool,
}

impl OutputStream for Claim {}

impl Drop for Claim {
    fn drop(&mut self) {
        unsafe {
            if self.started {
                AudioDeviceStop(self.device, self.proc_id);
            }
            // Destroying the IOProc guarantees it won't run again. It must come before
            // the box it reads is freed.
            if !self.proc_id.is_null() {
                AudioDeviceDestroyIOProcID(self.device, self.proc_id);
            }
            if !self.listener.is_null() {
                let address = address(DEVICE_IS_ALIVE, SCOPE_GLOBAL);
                AudioObjectRemovePropertyListener(
                    self.device,
                    &address,
                    alive_listener,
                    self.listener.cast(),
                );
                // Leaked on purpose. Remove doesn't promise an in-flight notification is
                // done with the client data, so freeing would race the notification thread.
                // One `Arc` per claim, and claims come from toggling a setting.
                std::mem::forget(Box::from_raw(self.listener));
            }
            if !self.state.is_null() {
                drop(Box::from_raw(self.state));
            }
            // Hog mode goes back last, or another app could reconfigure the device while
            // our IOProc is attached.
            if self.hogged {
                release_hog(self.device);
            }
        }
    }
}

// --- The seam -------------------------------------------------------------

/// Devices with no output are filtered out here, not at claim time.
pub fn devices() -> Vec<Device> {
    outputs().into_iter().map(|(_, device)| device).collect()
}

/// Errors here become a fallback to shared, so they say what failed.
pub fn open(request: &Request, shared: &Arc<Shared>) -> Result<OpenOutput, String> {
    let list = outputs();
    // A named device that's gone takes the system default, like the shared backend.
    let picked = request
        .device
        .as_deref()
        .and_then(|want| list.iter().find(|(_, d)| d.id == want))
        .or_else(|| {
            let default = unsafe { default_output() }?;
            list.iter().find(|(id, _)| *id == default)
        })
        .or_else(|| list.first())
        .ok_or("no CoreAudio output device")?;
    let (device, named) = (picked.0, picked.1.clone());

    unsafe { claim(request, shared, device, named) }
}

/// FFI ordering, top to bottom: hog, settle rate and buffer, attach.
unsafe fn claim(
    request: &Request,
    shared: &Arc<Shared>,
    device: AudioObjectID,
    named: Device,
) -> Result<OpenOutput, String> {
    let mut held = Claim {
        device,
        proc_id: ptr::null_mut(),
        started: false,
        state: ptr::null_mut(),
        listener: ptr::null_mut(),
        hogged: false,
    };

    // First: the step most likely to fail, before anything needs undoing.
    take_hog(device)?;
    held.hogged = true;

    let rate = set_rate(device, request.rate.unwrap_or(DEFAULT_RATE))?;
    let frames = set_buffer_frames(device, rate, request.period_ms)?;
    let channels = output_channels(device);
    if channels == 0 {
        return Err(format!("{} has no output channels", named.name));
    }
    check_float_format(device, &named.name)?;

    let (ring_frames, producer, ring, tap_tx, tap) = rings(rate);
    let scratch = vec![0.0f32; (frames as usize).max(SCRATCH_FLOOR) * channels as usize];
    let state = Box::into_raw(Box::new(State {
        shared: shared.clone(),
        ring,
        tap: tap_tx,
        scratch,
    }));
    held.state = state;

    let status = AudioDeviceCreateIOProcID(device, io_proc, state.cast(), &mut held.proc_id);
    if status != 0 || held.proc_id.is_null() {
        return Err(format!("attaching to {}: {}", named.name, text(status)));
    }

    // Best effort: without the listener it still plays, it just won't trip the
    // reopen path on unplug.
    let listener = Box::into_raw(Box::new(shared.clone()));
    let alive = address(DEVICE_IS_ALIVE, SCOPE_GLOBAL);
    if AudioObjectAddPropertyListener(device, &alive, alive_listener, listener.cast()) == 0 {
        held.listener = listener;
    } else {
        drop(Box::from_raw(listener));
    }

    let status = AudioDeviceStart(device, held.proc_id);
    if status != 0 {
        return Err(format!("starting {}: {}", named.name, text(status)));
    }
    held.started = true;

    Ok(OpenOutput {
        stream: Box::new(held),
        negotiated: Negotiated {
            mode: Mode::Exclusive,
            device: named.name,
            sample_rate: rate,
            channels: channels as u16,
            // An IOProc gets the stream's virtual format, 32-bit float on every recent
            // Mac (`check_float_format` confirms it). The HAL's virtual-to-physical step
            // in hog mode doesn't mix or resample, so we report what we write.
            format: "f32".into(),
            fallback: None,
        },
        sample_rate: rate,
        ring_frames,
        producer,
        tap,
    })
}

/// The id a [`Request`] names is the device UID: AudioObjectIDs are handed out
/// per boot and reused, UIDs are stable for the same hardware.
fn outputs() -> Vec<(AudioObjectID, Device)> {
    let address = address(HARDWARE_DEVICES, SCOPE_GLOBAL);
    let ids = unsafe { property_array::<AudioObjectID>(SYSTEM_OBJECT, &address, "device list") };
    let Ok(ids) = ids else { return Vec::new() };

    let mut out = Vec::new();
    for id in ids {
        if unsafe { output_channels(id) } == 0 {
            continue;
        }
        let Some(uid) = (unsafe { cfstring_property(id, DEVICE_UID) }) else {
            continue;
        };
        let name = unsafe { cfstring_property(id, OBJECT_NAME) }.unwrap_or_else(|| uid.clone());
        out.push((id, Device { id: uid, name }));
    }
    out
}

// --- Device properties ----------------------------------------------------

fn address(selector: u32, scope: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        selector,
        scope,
        element: ELEMENT_MAIN,
    }
}

/// Read a fixed-size property. A short write is an error, not a half-filled value.
///
/// # Safety
/// `T` has to be the type the SDK documents for `address` on this object.
unsafe fn property<T: Copy>(
    object: AudioObjectID,
    address: &AudioObjectPropertyAddress,
    what: &str,
) -> Result<T, String> {
    let mut value = MaybeUninit::<T>::uninit();
    let mut size = size_of::<T>() as u32;
    let status = AudioObjectGetPropertyData(
        object,
        address,
        0,
        ptr::null(),
        &mut size,
        value.as_mut_ptr().cast(),
    );
    if status != 0 {
        return Err(format!("{what}: {}", text(status)));
    }
    if size as usize != size_of::<T>() {
        return Err(format!(
            "{what}: the HAL wrote {size} bytes, not {}",
            size_of::<T>()
        ));
    }
    Ok(value.assume_init())
}

/// # Safety
/// Same as [`property`], and `T` has to be the array's element type.
unsafe fn property_array<T: Copy>(
    object: AudioObjectID,
    address: &AudioObjectPropertyAddress,
    what: &str,
) -> Result<Vec<T>, String> {
    let mut bytes: u32 = 0;
    let status = AudioObjectGetPropertyDataSize(object, address, 0, ptr::null(), &mut bytes);
    if status != 0 {
        return Err(format!("{what} size: {}", text(status)));
    }
    let count = bytes as usize / size_of::<T>();
    if count == 0 {
        return Ok(Vec::new());
    }

    let mut out: Vec<T> = Vec::with_capacity(count);
    let mut size = (count * size_of::<T>()) as u32;
    let status = AudioObjectGetPropertyData(
        object,
        address,
        0,
        ptr::null(),
        &mut size,
        out.as_mut_ptr().cast(),
    );
    if status != 0 {
        return Err(format!("{what}: {}", text(status)));
    }
    out.set_len((size as usize / size_of::<T>()).min(count));
    Ok(out)
}

/// CoreAudio returns these retained; we release.
///
/// # Safety
/// `selector` has to name a CFStringRef property on this object.
unsafe fn cfstring_property(object: AudioObjectID, selector: u32) -> Option<String> {
    let address = address(selector, SCOPE_GLOBAL);
    let string: *const c_void = property(object, &address, "name").ok()?;
    if string.is_null() {
        return None;
    }
    let out = cfstring_to_string(string);
    CFRelease(string);
    out
}

/// # Safety
/// `string` has to be a live CFStringRef.
unsafe fn cfstring_to_string(string: *const c_void) -> Option<String> {
    // Four UTF-8 bytes per UTF-16 unit at most, plus the terminator.
    let capacity = (CFStringGetLength(string).max(0) * 4 + 1) as usize;
    let mut buffer = vec![0u8; capacity];
    if CFStringGetCString(
        string,
        buffer.as_mut_ptr().cast::<c_char>(),
        capacity as c_long,
        CFSTRING_ENCODING_UTF8,
    ) == 0
    {
        return None;
    }
    let end = buffer.iter().position(|&b| b == 0).unwrap_or(capacity);
    buffer.truncate(end);
    String::from_utf8(buffer).ok()
}

/// # Safety
/// Nothing to hold; the system object is always there.
unsafe fn default_output() -> Option<AudioObjectID> {
    let address = address(HARDWARE_DEFAULT_OUTPUT, SCOPE_GLOBAL);
    property::<AudioObjectID>(SYSTEM_OBJECT, &address, "default output device").ok()
}

/// Total output channels across streams, from the IOProc's buffer layout.
/// Zero means not an output device.
///
/// # Safety
/// `device` has to be a live AudioObjectID.
unsafe fn output_channels(device: AudioObjectID) -> u32 {
    let address = address(DEVICE_STREAM_CONFIGURATION, SCOPE_OUTPUT);
    // Read as u64 for the 8-byte alignment the struct needs, which Vec<u8>
    // doesn't promise.
    let Ok(words) = property_array::<u64>(device, &address, "stream configuration") else {
        return 0;
    };
    let bytes = words.len() * size_of::<u64>();
    if bytes < size_of::<AudioBufferList>() {
        return 0;
    }
    let list = words.as_ptr().cast::<AudioBufferList>();
    let count = (*list).number_buffers as usize;
    let first = ptr::addr_of!((*list).buffers).cast::<AudioBuffer>();
    let header = first as usize - list as usize;
    if count == 0 || bytes < header + count * size_of::<AudioBuffer>() {
        return 0;
    }
    std::slice::from_raw_parts(first, count)
        .iter()
        .map(|buffer| buffer.number_channels)
        .sum()
}

/// [`render`] writes `f32` into the virtual format unconverted, so a device
/// that virtualizes as anything else fails the claim here and falls back to
/// shared. A format that won't read keeps the assumption.
///
/// # Safety
/// `device` has to be a live AudioObjectID.
unsafe fn check_float_format(device: AudioObjectID, name: &str) -> Result<(), String> {
    let streams = address(DEVICE_STREAMS, SCOPE_OUTPUT);
    let Ok(ids) = property_array::<AudioObjectID>(device, &streams, "output streams") else {
        return Ok(());
    };
    let Some(stream) = ids.first().copied() else {
        return Ok(());
    };
    let virtual_format = address(STREAM_VIRTUAL_FORMAT, SCOPE_GLOBAL);
    let Ok(format) = property::<StreamFormat>(stream, &virtual_format, "virtual format") else {
        return Ok(());
    };
    if format.format_id == FORMAT_LINEAR_PCM
        && format.format_flags & FORMAT_FLAG_IS_FLOAT != 0
        && format.bits_per_channel == 32
    {
        return Ok(());
    }
    Err(format!(
        "{name} renders a format rox writes only through the mixer ({} bit, id {:#010x})",
        format.bits_per_channel, format.format_id
    ))
}

// --- Hog mode -------------------------------------------------------------

/// Take hog mode, or report who has it.
///
/// `AudioHardware.h`: the value passed to a hog mode set is ignored and the
/// set toggles. So never set it while we already own it, or it's released.
/// The read-back decides whether we got it.
///
/// # Safety
/// `device` has to be a live AudioObjectID.
unsafe fn take_hog(device: AudioObjectID) -> Result<(), String> {
    let address = address(DEVICE_HOG_MODE, SCOPE_GLOBAL);
    let me = getpid();
    let owner = property::<Pid>(device, &address, "hog mode")?;
    if owner == me {
        // Ours already, from a claim whose Drop hasn't run. rox takes hog mode in
        // one place only.
        return Ok(());
    }
    if owner != NOBODY {
        return Err(format!("another app (pid {owner}) has the device"));
    }

    let status = AudioObjectSetPropertyData(
        device,
        &address,
        0,
        ptr::null(),
        size_of::<Pid>() as u32,
        ptr::addr_of!(me).cast(),
    );
    if status != 0 {
        return Err(format!("taking hog mode: {}", text(status)));
    }
    let owner = property::<Pid>(device, &address, "hog mode")?;
    if owner != me {
        return Err(format!("hog mode went to pid {owner} instead"));
    }
    Ok(())
}

/// # Safety
/// `device` has to be a live AudioObjectID.
unsafe fn release_hog(device: AudioObjectID) {
    let address = address(DEVICE_HOG_MODE, SCOPE_GLOBAL);
    let nobody = NOBODY;
    let status = AudioObjectSetPropertyData(
        device,
        &address,
        0,
        ptr::null(),
        size_of::<Pid>() as u32,
        ptr::addr_of!(nobody).cast(),
    );
    if status != 0 {
        // A device stuck hogged reads as "no other app has sound", so leave a trace.
        log::error!("exclusive output: releasing hog mode: {}", text(status));
    }
}

// --- Rate and buffer ------------------------------------------------------

/// Settle on the requested rate where the device has it, and report what it
/// actually settled at; the engine resamples the difference.
///
/// # Safety
/// `device` has to be a live AudioObjectID.
unsafe fn set_rate(device: AudioObjectID, want: u32) -> Result<u32, String> {
    let nominal = address(DEVICE_NOMINAL_SAMPLE_RATE, SCOPE_GLOBAL);
    let available = address(DEVICE_AVAILABLE_SAMPLE_RATES, SCOPE_GLOBAL);
    let ranges = property_array::<AudioValueRange>(device, &available, "available rates")
        .unwrap_or_default();
    let target = pick_rate(&ranges, want as f64);

    let current = property::<f64>(device, &nominal, "nominal sample rate")?;
    if !same_rate(current, target) {
        let status = AudioObjectSetPropertyData(
            device,
            &nominal,
            0,
            ptr::null(),
            size_of::<f64>() as u32,
            ptr::addr_of!(target).cast(),
        );
        if status != 0 {
            return Err(format!("setting rate {target}: {}", text(status)));
        }
        // The set returns before the driver relocks, so poll. This blocks `open`'s
        // caller, so it stops the moment the rate settles.
        for _ in 0..RATE_SETTLE_POLLS {
            std::thread::sleep(RATE_SETTLE_STEP);
            match property::<f64>(device, &nominal, "nominal sample rate") {
                Ok(now) if same_rate(now, target) => break,
                Ok(_) => continue,
                Err(e) => return Err(e),
            }
        }
    }

    let settled = property::<f64>(device, &nominal, "nominal sample rate")?;
    if settled < 1.0 {
        return Err(format!("device reports a {settled} Hz clock"));
    }
    Ok(settled.round() as u32)
}

/// Set the requested period and report the buffer the device took. No
/// period leaves the driver's tuned default alone.
///
/// # Safety
/// `device` has to be a live AudioObjectID.
unsafe fn set_buffer_frames(
    device: AudioObjectID,
    rate: u32,
    period_ms: Option<f64>,
) -> Result<u32, String> {
    let size = address(DEVICE_BUFFER_FRAME_SIZE, SCOPE_GLOBAL);
    if let Some(period_ms) = period_ms {
        let range = property::<AudioValueRange>(
            device,
            &address(DEVICE_BUFFER_FRAME_SIZE_RANGE, SCOPE_GLOBAL),
            "buffer frame size range",
        )
        .ok();
        let want = frames_for_period(rate, period_ms, range);
        // Not fatal: some drivers refuse a fixed buffer, and the read-back reports
        // what's running.
        let status = AudioObjectSetPropertyData(
            device,
            &size,
            0,
            ptr::null(),
            size_of::<u32>() as u32,
            ptr::addr_of!(want).cast(),
        );
        if status != 0 {
            log::warn!(
                "exclusive output: buffer of {want} frames refused: {}",
                text(status)
            );
        }
    }
    property::<u32>(device, &size, "buffer frame size")
}

/// Discrete rates come back as zero-width ranges. No list passes the request through.
fn pick_rate(ranges: &[AudioValueRange], want: f64) -> f64 {
    if ranges.is_empty() {
        return want;
    }
    let mut best = want;
    let mut best_gap = f64::INFINITY;
    for range in ranges {
        let (low, high) = (
            range.minimum.min(range.maximum),
            range.maximum.max(range.minimum),
        );
        let candidate = want.max(low).min(high);
        let gap = (candidate - want).abs();
        if gap < best_gap {
            best_gap = gap;
            best = candidate;
        }
    }
    best
}

/// Not `clamp`, which panics on a range reported backwards.
fn frames_for_period(rate: u32, period_ms: f64, range: Option<AudioValueRange>) -> u32 {
    let frames = (rate as f64 * period_ms / 1000.0).round().max(1.0);
    let frames = match range {
        Some(range) => frames.max(range.minimum).min(range.maximum),
        None => frames,
    };
    frames.max(1.0) as u32
}

/// Drivers round, so a hertz of slack is one clock.
fn same_rate(a: f64, b: f64) -> bool {
    (a - b).abs() < 1.0
}

// --- Render ---------------------------------------------------------------

/// On the HAL's real-time thread: no allocation, no lock, no logging, no I/O (ADR 2).
///
/// # Safety
/// Called by CoreAudio with the client data we handed
/// `AudioDeviceCreateIOProcID`, which stays alive until the IOProc is
/// destroyed in [`Claim::drop`].
unsafe extern "C" fn io_proc(
    _device: AudioObjectID,
    _now: AudioTimeStampRef,
    _input_data: *const AudioBufferList,
    _input_time: AudioTimeStampRef,
    output_data: *mut AudioBufferList,
    _output_time: AudioTimeStampRef,
    client_data: *mut c_void,
) -> OSStatus {
    if output_data.is_null() || client_data.is_null() {
        return 0;
    }
    render(&mut *client_data.cast::<State>(), output_data);
    0
}

/// # Safety
/// `output` has to be the live AudioBufferList the HAL just handed us.
unsafe fn render(state: &mut State, output: *mut AudioBufferList) {
    let count = (*output).number_buffers as usize;
    if count == 0 {
        return;
    }
    let buffers = std::slice::from_raw_parts_mut(
        ptr::addr_of_mut!((*output).buffers).cast::<AudioBuffer>(),
        count,
    );

    // One interleaved buffer: every built-in output and nearly every USB DAC.
    if count == 1 {
        let buffer = &buffers[0];
        let channels = buffer.number_channels as usize;
        if buffer.data.is_null() || channels == 0 {
            return;
        }
        // Whole frames only, or `fill` indexes past a short tail.
        let samples = (buffer.data_byte_size as usize / size_of::<f32>()) / channels * channels;
        if samples == 0 {
            return;
        }
        let data = std::slice::from_raw_parts_mut(buffer.data.cast::<f32>(), samples);
        fill(
            data,
            channels,
            &state.shared,
            &mut state.ring,
            &mut state.tap,
        );
        return;
    }

    // Deinterleaved (pro interfaces, aggregates): fill the scratch sized at
    // open, then scatter. Silence first so uncovered buffers aren't stale.
    let mut total = 0usize;
    let mut frames = usize::MAX;
    for buffer in buffers.iter() {
        let channels = buffer.number_channels as usize;
        total += channels;
        if buffer.data.is_null() {
            frames = 0;
            continue;
        }
        // Zero before judging, so a skipped buffer goes out silent.
        let samples = buffer.data_byte_size as usize / size_of::<f32>();
        std::slice::from_raw_parts_mut(buffer.data.cast::<f32>(), samples).fill(0.0);
        if channels == 0 {
            frames = 0;
            continue;
        }
        frames = frames.min(samples / channels);
    }
    if total == 0 || frames == 0 || frames == usize::MAX {
        return;
    }
    let frames = frames.min(state.scratch.len() / total);
    if frames == 0 {
        return;
    }

    let mixed = &mut state.scratch[..frames * total];
    fill(mixed, total, &state.shared, &mut state.ring, &mut state.tap);

    let mut base = 0usize;
    for buffer in buffers.iter() {
        let channels = buffer.number_channels as usize;
        let data = std::slice::from_raw_parts_mut(buffer.data.cast::<f32>(), frames * channels);
        for frame in 0..frames {
            for channel in 0..channels {
                data[frame * channels + channel] = mixed[frame * total + base + channel];
            }
        }
        base += channels;
    }
}

/// On the notification thread, so logging is fine.
///
/// # Safety
/// Called by CoreAudio with the boxed `Arc<Shared>` from [`claim`], which
/// outlives the listener registration.
unsafe extern "C" fn alive_listener(
    device: AudioObjectID,
    _number_addresses: u32,
    _addresses: *const AudioObjectPropertyAddress,
    client_data: *mut c_void,
) -> OSStatus {
    if client_data.is_null() {
        return 0;
    }
    let shared = &*client_data.cast::<Arc<Shared>>();
    let address = address(DEVICE_IS_ALIVE, SCOPE_GLOBAL);
    // A failed read counts as death.
    let alive = property::<u32>(device, &address, "device is alive").unwrap_or(0);
    if alive == 0 {
        log::error!("exclusive output: the device went away");
        shared.device_lost.store(true, Ordering::Release);
    }
    0
}

// --- Errors ---------------------------------------------------------------

/// OSStatus as its four-char code (`'!obj'`, `'nope'`), since these end up on
/// the settings page as the reason exclusive fell back.
fn text(status: OSStatus) -> String {
    let bytes = (status as u32).to_be_bytes();
    if bytes.iter().all(|b| (0x20..=0x7e).contains(b)) {
        let code: String = bytes.iter().map(|&b| b as char).collect();
        format!("OSStatus {status} '{code}'")
    } else {
        format!("OSStatus {status}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(minimum: f64, maximum: f64) -> AudioValueRange {
        AudioValueRange { minimum, maximum }
    }

    #[test]
    fn four_char_statuses_read_as_their_code() {
        // 'what', kAudioHardwareIllegalOperationError.
        assert_eq!(text(0x7768_6174), "OSStatus 2003329396 'what'");
        assert_eq!(text(-1), "OSStatus -1");
        assert_eq!(text(0), "OSStatus 0");
    }

    #[test]
    fn a_rate_the_device_lists_comes_back_untouched() {
        let discrete = [
            range(44100.0, 44100.0),
            range(48000.0, 48000.0),
            range(96000.0, 96000.0),
        ];
        assert_eq!(pick_rate(&discrete, 48000.0), 48000.0);
        assert_eq!(pick_rate(&[range(44100.0, 192_000.0)], 88200.0), 88200.0);
    }

    #[test]
    fn a_rate_the_device_lacks_lands_on_the_nearest_one() {
        let discrete = [range(44100.0, 44100.0), range(48000.0, 48000.0)];
        assert_eq!(pick_rate(&discrete, 96000.0), 48000.0);
        assert_eq!(pick_rate(&discrete, 8000.0), 44100.0);
        assert_eq!(pick_rate(&[], 176_400.0), 176_400.0);
    }

    #[test]
    fn periods_become_frames_inside_the_devices_range() {
        assert_eq!(frames_for_period(48000, 10.0, None), 480);
        let allowed = Some(range(64.0, 4096.0));
        assert_eq!(frames_for_period(48000, 10.0, allowed), 480);
        assert_eq!(frames_for_period(48000, 0.1, allowed), 64);
        assert_eq!(frames_for_period(192_000, 500.0, allowed), 4096);
        assert_eq!(
            frames_for_period(48000, 10.0, Some(range(4096.0, 64.0))),
            64
        );
    }
}
