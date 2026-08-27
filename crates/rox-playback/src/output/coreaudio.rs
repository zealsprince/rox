//! Exclusive output on macOS: the device taken in hog mode, which is the one
//! thing CoreAudio offers that means "this process and nobody else". ADR 19
//! names it directly, and it's the only route with the property the mode
//! exists for: with the HAL's mixer out of the path, the nominal sample rate
//! we set is the rate the converter runs at instead of a number the mixer
//! resamples toward.
//!
//! The shape matches [`super::alsa`] where the platform allows, but the
//! render loop is inverted twice over. ALSA blocks in `writei` on a thread we
//! own; CoreAudio is a pull model like cpal, so there's no writer thread here
//! at all. The HAL calls [`io_proc`] on its own real-time thread and we hand
//! the same [`fill`] the same buffer, which is the part of the contract ADR
//! 19 says every backend keeps.
//!
//! Every SDK constant below is declared by hand rather than pulled from
//! `coreaudio-sys`. That crate runs bindgen over the macOS SDK headers, which
//! only exist on a Mac, and this file has to type-check from a cross build on
//! any host. The surface is small enough that the trade is worth it: the
//! four-char code of every selector is in its doc comment, so a constant that
//! drifts from the header is one `grep` in `AudioHardware.h` away from being
//! caught.

#![allow(non_snake_case)]

use std::ffi::{c_char, c_long, c_void};
use std::mem::MaybeUninit;
use std::ptr;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use rtrb::{Consumer, Producer};

use super::{fill, rings, Device, Mode, Negotiated, OpenOutput, OutputStream, Request};
use crate::shared::Shared;

/// The rate to ask for when the caller doesn't name one, which is every
/// session that opens before a file has been decoded. The pump reopens at the
/// file's own rate once it knows it.
const DEFAULT_RATE: u32 = 48000;

/// How long to wait for a nominal rate change to settle. Setting the rate is
/// asynchronous: the call returns while the HAL is still telling the driver,
/// and reading straight back gets the old rate. A hundred polls of 5 ms is
/// still the half second the USB interfaces that relock their clock need, but
/// the step is short because the wait blocks whoever called `open`: a built-in
/// output that settles immediately costs one step rather than twenty times
/// that.
const RATE_SETTLE_POLLS: u32 = 100;
const RATE_SETTLE_STEP: Duration = Duration::from_millis(5);

/// The pid the HAL reports when no process owns the device.
const NOBODY: Pid = -1;

/// Scratch frames to keep in hand for the deinterleaved render path when the
/// device won't report how big its buffer is. Only a floor; the real size comes
/// from the buffer frame size we read back.
const SCRATCH_FLOOR: usize = 4096;

// --- CoreAudio types ------------------------------------------------------

type OSStatus = i32;
type AudioObjectID = u32;
/// `pid_t`, spelled in camel case so it doesn't trip the style lint.
type Pid = i32;

/// `AudioDeviceIOProcID` in the header is a typedef of the IOProc function
/// type, and the HAL hands back a token rather than the pointer we passed. We
/// only ever store it and give it back, so an opaque pointer is the honest
/// spelling and it's the same size either way.
type AudioDeviceIOProcID = *mut c_void;

/// `const AudioTimeStamp*` in the header. rox never reads one, so the struct
/// stays out of this file instead of being copied from the SDK (it contains
/// an SMPTETime, and a field wrong there would be a silent ABI bug for no
/// gain).
type AudioTimeStampRef = *const c_void;

/// The IOProc the HAL calls per buffer.
type AudioDeviceIOProc = unsafe extern "C" fn(
    device: AudioObjectID,
    now: AudioTimeStampRef,
    input_data: *const AudioBufferList,
    input_time: AudioTimeStampRef,
    output_data: *mut AudioBufferList,
    output_time: AudioTimeStampRef,
    client_data: *mut c_void,
) -> OSStatus;

/// The proc the HAL calls when a watched property changes.
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

/// C's flexible array trick: the struct declares one [`AudioBuffer`] and the
/// HAL hands over storage for `number_buffers` of them. Never construct one
/// by value; it's only ever read through a pointer the HAL owns, and the
/// buffers are addressed with pointer arithmetic from `buffers` rather than by
/// indexing the one-element array.
#[repr(C)]
struct AudioBufferList {
    /// `mNumberBuffers`
    number_buffers: u32,
    /// `mBuffers`
    buffers: [AudioBuffer; 1],
}

/// `AudioStreamBasicDescription`, how CoreAudio spells a format. Every field
/// is declared because the HAL writes the whole struct and [`property`] checks
/// the byte count it wrote; only three are read.
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
extern "C" {
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
extern "C" {
    fn CFStringGetLength(string: *const c_void) -> c_long;
    fn CFStringGetCString(
        string: *const c_void,
        buffer: *mut c_char,
        buffer_size: c_long,
        encoding: u32,
    ) -> u8;
    fn CFRelease(object: *const c_void);
}

extern "C" {
    /// From libSystem, which every macOS binary links anyway. One extern
    /// declaration is cheaper than taking a `libc` dependency for a single
    /// call that has been in the same place since System V.
    fn getpid() -> Pid;
}

// --- The claim ------------------------------------------------------------

/// What the IOProc needs to render, boxed once at open and accessed through
/// a raw pointer because the HAL's client data is a `void*`. Nothing here
/// allocates or locks when it's used, which is the whole reason the scratch
/// buffer is sized up front.
struct State {
    shared: Arc<Shared>,
    ring: Consumer<f32>,
    tap: Producer<f32>,
    /// Interleaved staging for devices that hand out one buffer per stream.
    /// Untouched on the common single-buffer path.
    scratch: Vec<f32>,
}

/// The claim on the device. Dropping it stops the IOProc, unregisters it,
/// drops what the callback was reading, and hands hog mode back, so
/// toggling exclusive off actually releases the device.
///
/// Built empty and filled in as `open` gets further, so a failure halfway
/// through unwinds through this same Drop instead of needing its own cleanup
/// path per step. Every field is checked before it's used, so a Claim that
/// never got past hog mode drops cleanly.
struct Claim {
    device: AudioObjectID,
    proc_id: AudioDeviceIOProcID,
    started: bool,
    state: *mut State,
    /// The `Arc<Shared>` handed to the alive listener. Its own allocation
    /// rather than a borrow of `state`: the listener runs on the HAL's
    /// notification thread while the IOProc is mutating the rings, and two
    /// threads pointing into one box would be aliasing we'd have to argue
    /// our way out of.
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
            // Destroying the IOProc is the call that guarantees it isn't
            // running and won't be called again. It has to come before the
            // box it reads is freed, which is the one ordering in here that
            // can't be relaxed.
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
                // And the box stays. Remove coming back isn't a promise that
                // a notification already in flight is done with the client
                // data, the way DestroyIOProcID above is for the IOProc's, so
                // freeing here would race the HAL's notification thread onto
                // memory it's reading. A pointer and one strong count on
                // Shared per claim is a cheaper price than that race, and a
                // claim happens when the user toggles a setting, not per
                // track.
                std::mem::forget(Box::from_raw(self.listener));
            }
            if !self.state.is_null() {
                drop(Box::from_raw(self.state));
            }
            // Hog mode goes back last. Releasing it earlier would let another
            // app claim the device and reconfigure its rate while our IOProc
            // was still attached.
            if self.hogged {
                release_hog(self.device);
            }
        }
    }
}

// --- The seam -------------------------------------------------------------

/// Every device the HAL exposes that has an output stream. Devices with no
/// output (microphones, the input half of an aggregate) are filtered out
/// here rather than failing later at claim time.
pub fn devices() -> Vec<Device> {
    outputs().into_iter().map(|(_, device)| device).collect()
}

/// Claim a device and start its IOProc. Every error here is one the seam
/// turns into a fallback to shared output, so they say what failed rather
/// than just that something did.
pub fn open(request: &Request, shared: &Arc<Shared>) -> Result<OpenOutput, String> {
    let list = outputs();
    // A named device that's gone (interface unplugged, UID changed by a
    // driver update) takes the system default rather than failing the open,
    // matching what the shared backend does with a stale cpal name.
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

/// The unsafe half of [`open`], kept in one place so the FFI ordering reads
/// top to bottom: hog the device, settle its rate and buffer, then attach.
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

    // First, because it's the step that makes this exclusive at all and the
    // one most likely to fail. Everything after it is configuration we'd have
    // to undo if the claim itself failed.
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

    // Best effort: a device that won't let us watch it for death still plays,
    // it just won't trip the app's reopen path when it's unplugged. Not worth
    // failing a working claim over.
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
            // The simplification this backend takes knowingly: an IOProc is
            // handed the stream's *virtual* format, and on every Mac shipped
            // this decade that's 32-bit float, which `check_float_format`
            // above confirms before we get here. Chasing the physical format
            // (`kAudioStreamPropertyPhysicalFormat`, then matching integer
            // widths) would buy nothing audible, because the HAL's
            // virtual-to-physical step in hog mode is a straight conversion
            // with no mixing or resampling in it. So we report what we
            // actually write.
            format: "f32".into(),
            fallback: None,
        },
        sample_rate: rate,
        ring_frames,
        producer,
        tap,
    })
}

/// Devices paired with their AudioObjectID, which `open` needs and the picker
/// doesn't. The id a [`Request`] names is the device UID, not the
/// AudioObjectID: object ids are handed out per boot and get reused, so a
/// saved id would point at whatever device happened to take that slot next
/// time. The UID is the string the HAL promises is stable for the same
/// hardware, which is what a setting needs to mean anything after a reboot.
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

/// Read a property whose size the SDK fixes. The size going in is what we
/// expect and the size coming back is what the HAL wrote, so a short write
/// reads as an error instead of a half-filled value.
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

/// Read a property that's an array: ask the size first, then fill.
///
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

/// A CFString-valued property as a Rust String. CoreAudio hands these back
/// retained, so the release is ours to make.
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
    // UTF-16 code units in, UTF-8 bytes out: four bytes per unit is the
    // ceiling, plus the terminator.
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

/// The device macOS is currently sending everything to.
///
/// # Safety
/// Nothing to hold; the system object is always there.
unsafe fn default_output() -> Option<AudioObjectID> {
    let address = address(HARDWARE_DEFAULT_OUTPUT, SCOPE_GLOBAL);
    property::<AudioObjectID>(SYSTEM_OBJECT, &address, "default output device").ok()
}

/// Total output channels across the device's streams, read from the same
/// buffer layout the IOProc will be handed. Zero means this isn't an output
/// device (or the HAL wouldn't report it), and the caller skips it.
///
/// # Safety
/// `device` has to be a live AudioObjectID.
unsafe fn output_channels(device: AudioObjectID) -> u32 {
    let address = address(DEVICE_STREAM_CONFIGURATION, SCOPE_OUTPUT);
    // An AudioBufferList is 8 bytes of header plus 16 per buffer on 64-bit,
    // so reading it as u64 both covers it exactly and gets the 8-byte
    // alignment the struct needs, which a Vec<u8> wouldn't promise.
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

/// Check the backend's one assumption instead of leaving it implicit: the
/// IOProc is handed the stream's virtual format, and [`render`] writes `f32`
/// into those buffers with no conversion at all. A device that virtualizes
/// as anything else would take that as garbage, so it fails the claim here
/// and the seam falls back to shared, which handles any format cpal knows.
///
/// A device whose format won't read keeps the assumption. The property is on
/// the stream object rather than the device, and a driver that hides its
/// streams shouldn't lose exclusive over a read we only wanted as a check.
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

/// Take exclusive ownership, or report who has it.
///
/// The API here is odd enough to be worth stating: `AudioHardware.h` says the
/// value passed to a hog mode *set* is ignored, and the set toggles. Nobody
/// owns it and you gain it; you own it and you give it up. So the guard below
/// isn't defensive tidiness, it's load-bearing: setting while we already own
/// the device would release it. We write our own pid to take and -1 to
/// release anyway, so the call reads right under either interpretation, and
/// the read-back decides whether we got it.
///
/// # Safety
/// `device` has to be a live AudioObjectID.
unsafe fn take_hog(device: AudioObjectID) -> Result<(), String> {
    let address = address(DEVICE_HOG_MODE, SCOPE_GLOBAL);
    let me = getpid();
    let owner = property::<Pid>(device, &address, "hog mode")?;
    if owner == me {
        // Ours already, from a claim whose Drop hasn't run yet. Treat it
        // as ours to release: rox takes hog mode in exactly one place, so
        // there's no other holder in this process to steal it from.
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
        // Nothing to do about it from a Drop, but a device stuck hogged is
        // the kind of thing a user reports as "no other app has sound", so
        // leave a trace.
        log::error!("exclusive output: releasing hog mode: {}", text(status));
    }
}

// --- Rate and buffer ------------------------------------------------------

/// Put the device on the requested rate where it has one, and report what it
/// actually settled at. Same honesty rule as ALSA's `set_rate_near`: a card
/// that won't do 96 kHz reads as the rate it does, not the rate rox wished
/// for, and the engine's resampler covers the difference.
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
        // The set returns before the driver has relocked, so poll rather than
        // read straight back. Off the real-time path by construction: this is
        // open, and the IOProc doesn't exist yet. It does hold up the thread
        // that called open, which today is the UI's, so the loop ends the
        // moment the device settles instead of on a fixed tick.
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

/// Ask for the requested period and report the buffer the device took, which
/// sizes the scratch staging. A caller that names no period leaves
/// the device's own buffer alone: CoreAudio's default is already tuned per
/// driver, and overriding it with a guess would trade latency for dropouts
/// nobody asked for.
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
        // Not fatal: some drivers publish a fixed buffer and refuse the
        // write. The read-back below reports what's really running, and the
        // period knob is a preference, not the point of the mode.
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

/// The nearest rate the device admits to having. Discrete rates come back as
/// ranges with equal ends, continuous ones as real spans, and a device that
/// lists neither gets the request passed through for the HAL to judge.
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

/// Frames per period for a millisecond request, clamped to what the device
/// allows. Order matters over `clamp`, which panics on a driver that reports
/// its range backwards.
fn frames_for_period(rate: u32, period_ms: f64, range: Option<AudioValueRange>) -> u32 {
    let frames = (rate as f64 * period_ms / 1000.0).round().max(1.0);
    let frames = match range {
        Some(range) => frames.max(range.minimum).min(range.maximum),
        None => frames,
    };
    frames.max(1.0) as u32
}

/// Sample rates are Float64 and drivers round, so 44100 and 44100.0000001 are
/// the same clock. A whole hertz of slack is well under any real difference.
fn same_rate(a: f64, b: f64) -> bool {
    (a - b).abs() < 1.0
}

// --- Render ---------------------------------------------------------------

/// The HAL's per-buffer callback, on its own real-time thread. Obeys ADR 2
/// the same way cpal's does: no allocation, no lock, no logging, no I/O.
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

    // The case every built-in output and nearly every USB DAC takes: one
    // buffer with the channels interleaved in it, which is exactly the shape
    // `fill` writes.
    if count == 1 {
        let buffer = &buffers[0];
        let channels = buffer.number_channels as usize;
        if buffer.data.is_null() || channels == 0 {
            return;
        }
        // Trim to whole frames. A tail shorter than one frame would index
        // `frame[1]` on a chunk of length one inside `fill`.
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

    // Deinterleaved: one buffer per stream, which is how most pro interfaces
    // and every aggregate device present themselves. `fill` only writes
    // interleaved, so it fills the scratch that was sized at open and we
    // scatter from there. Silence first, so any buffer the scatter can't
    // cover is quiet rather than stale.
    let mut total = 0usize;
    let mut frames = usize::MAX;
    for buffer in buffers.iter() {
        let channels = buffer.number_channels as usize;
        total += channels;
        if buffer.data.is_null() {
            frames = 0;
            continue;
        }
        // Zeroing comes before the frame count is judged, so a buffer we
        // later decide not to scatter into still goes out silent instead of
        // replaying whatever the HAL left in it.
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

/// The HAL telling us the device changed state. Runs on its notification
/// thread, not the IOProc's, so logging is fine here.
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
    // A device we can't even query is a device that's gone, so a failed read
    // counts as death rather than being ignored.
    let alive = property::<u32>(device, &address, "device is alive").unwrap_or(0);
    if alive == 0 {
        log::error!("exclusive output: the device went away");
        shared.device_lost.store(true, Ordering::Release);
    }
    0
}

// --- Errors ---------------------------------------------------------------

/// An OSStatus in the spelling the headers use. CoreAudio's errors are
/// four-char codes stuffed into an integer (`'!obj'`, `'stop'`, `'nope'`),
/// and the number alone sends nobody anywhere. These strings end up in the
/// settings page as the reason exclusive fell back, so they're worth the
/// twelve lines.
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
        // Nothing printable in there, so the number stands alone.
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
        // A device that lists nothing gets the request passed through.
        assert_eq!(pick_rate(&[], 176_400.0), 176_400.0);
    }

    #[test]
    fn periods_become_frames_inside_the_devices_range() {
        assert_eq!(frames_for_period(48000, 10.0, None), 480);
        let allowed = Some(range(64.0, 4096.0));
        assert_eq!(frames_for_period(48000, 10.0, allowed), 480);
        // Below the floor and above the ceiling both clamp.
        assert_eq!(frames_for_period(48000, 0.1, allowed), 64);
        assert_eq!(frames_for_period(192_000, 500.0, allowed), 4096);
        // A driver reporting its range backwards must not panic.
        assert_eq!(
            frames_for_period(48000, 10.0, Some(range(4096.0, 64.0))),
            64
        );
    }
}
