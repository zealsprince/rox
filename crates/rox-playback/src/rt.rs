//! Real-time scheduling for the threads that write to a device (ADR 19). A
//! writer held off by an ordinary scheduling delay longer than its buffer is
//! an xrun, so each one asks for real-time priority as it starts. A refusal
//! is logged and lived with: audio still plays, at normal priority.
//!
//! Linux asks RTKit over the system bus through zbus, pure Rust like the
//! MPRIS side, so no libdbus enters the build. Windows joins the MMCSS "Pro
//! Audio" class through `audio_thread_priority`. macOS needs nothing: the
//! HAL owns the IO thread in both modes.

/// Promote the calling thread. `period_frames` at `rate` is the slice the
/// scheduler is told the thread works in. RTKit takes no slice, so only
/// Windows reads them.
#[cfg_attr(target_os = "linux", allow(unused_variables))]
pub fn promote_current(period_frames: u32, rate: u32) {
    #[cfg(target_os = "linux")]
    report(linux::promote_current());

    #[cfg(target_os = "windows")]
    report(windows::promote_current(period_frames, rate));
}

/// Promote another thread of this process by its name. For the worker cpal
/// spawns in shared mode and only promotes behind a libdbus-backed feature.
#[cfg(target_os = "linux")]
pub fn promote_named(name: &str) {
    match linux::promote_named(name) {
        Ok(()) => log::info!("audio thread {name:?} runs real-time"),
        Err(e) => log::warn!("audio thread {name:?} stays at normal priority: {e}"),
    }
}

fn report(result: Result<(), String>) {
    let thread = std::thread::current();
    let name = thread.name().unwrap_or("unnamed");
    match result {
        Ok(()) => log::info!("audio thread {name:?} runs real-time"),
        Err(e) => log::warn!("audio thread {name:?} stays at normal priority: {e}"),
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use zbus::blocking::{Connection, Proxy};

    const RTKIT: &str = "org.freedesktop.RealtimeKit1";
    const RTKIT_PATH: &str = "/org/freedesktop/RealtimeKit1";
    /// What PulseAudio and Firefox ask for. RTKit clamps to its own maximum.
    const PRIORITY: i32 = 10;

    pub fn promote_current() -> Result<(), String> {
        // SAFETY: gettid takes no arguments and can't fail.
        let tid = unsafe { libc::syscall(libc::SYS_gettid) };
        let tid = u64::try_from(tid).map_err(|_| "gettid failed".to_string())?;

        promote(tid)
    }

    /// Matched on `comm`, which the kernel truncates to 15 bytes.
    pub fn promote_named(name: &str) -> Result<(), String> {
        let comm: String = name.chars().take(15).collect();
        let tasks =
            std::fs::read_dir("/proc/self/task").map_err(|e| format!("/proc/self/task: {e}"))?;

        for task in tasks.flatten() {
            let Ok(found) = std::fs::read_to_string(task.path().join("comm")) else {
                continue;
            };
            if found.trim_end() != comm {
                continue;
            }

            let tid = task
                .file_name()
                .to_string_lossy()
                .parse::<u64>()
                .map_err(|e| format!("task id: {e}"))?;

            return promote(tid);
        }

        Err(format!("no thread named {name:?}"))
    }

    fn promote(tid: u64) -> Result<(), String> {
        let conn = Connection::system().map_err(|e| format!("system bus: {e}"))?;
        let rtkit =
            Proxy::new(&conn, RTKIT, RTKIT_PATH, RTKIT).map_err(|e| format!("rtkit: {e}"))?;
        let max_prio: i32 = rtkit
            .get_property("MaxRealtimePriority")
            .map_err(|e| format!("rtkit limits: {e}"))?;
        let max_rttime: i64 = rtkit
            .get_property("RTTimeUSecMax")
            .map_err(|e| format!("rtkit limits: {e}"))?;

        // RTKit refuses a process whose RLIMIT_RTTIME is unlimited: the limit is
        // what stops a runaway real-time thread from taking the machine. Both
        // bounds at its maximum, as PipeWire sets them. A writer that blocks every
        // period never gets near it; one that spins past it is killed, which is
        // the deal every real-time audio process on Linux takes.
        let max_rttime =
            libc::rlim_t::try_from(max_rttime).map_err(|_| "rtkit: bad RTTimeUSecMax")?;
        let limit = libc::rlimit {
            rlim_cur: max_rttime,
            rlim_max: max_rttime,
        };
        // SAFETY: a complete rlimit for this process.
        if unsafe { libc::setrlimit(libc::RLIMIT_RTTIME, &limit) } != 0 {
            return Err(format!("setrlimit: {}", std::io::Error::last_os_error()));
        }

        let prio = u32::try_from(PRIORITY.min(max_prio).max(1)).unwrap_or(1);
        rtkit
            .call_method("MakeThreadRealtime", &(tid, prio))
            .map_err(|e| format!("rtkit: {e}"))?;

        Ok(())
    }
}

#[cfg(target_os = "windows")]
mod windows {
    pub fn promote_current(period_frames: u32, rate: u32) -> Result<(), String> {
        // The handle only matters for demotion, which a thread that lives as long
        // as its stream never does.
        audio_thread_priority::promote_current_thread_to_real_time(period_frames, rate)
            .map(|_handle| ())
            .map_err(|e| e.to_string())
    }
}
