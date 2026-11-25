//! SIGTERM, SIGINT and SIGHUP quit the way the Quit action does, so a
//! logout or a `kill` runs every quit hook: the pending layout save, the
//! settings flush, the plugin and renderer teardown. The default action
//! would end the process on the spot and skip them all.
//!
//! The handler only writes a byte to a pipe. A thread waits on the other
//! end and hands the request to the foreground, which dispatches `Quit`.
//! The first signal puts all three back to their default action, so a
//! second one still ends a rox whose quit hangs. A signal ignored at launch
//! (`nohup`, a background job) stays ignored. Windows has none of these
//! signals and gets nothing here.

use gpui::App;

#[cfg(unix)]
pub fn install(cx: &mut App) {
    unix::install(cx);
}

#[cfg(not(unix))]
pub fn install(_: &mut App) {}

#[cfg(unix)]
mod unix {
    use std::io::ErrorKind;
    use std::sync::atomic::{AtomicI32, Ordering};

    use gpui::App;
    use libc::c_int;

    use crate::workspace::Quit;

    const SIGNALS: [c_int; 3] = [libc::SIGTERM, libc::SIGINT, libc::SIGHUP];

    /// The pipe's write end, for the handler. Negative until installed.
    static WAKE: AtomicI32 = AtomicI32::new(-1);

    pub(super) fn install(cx: &mut App) {
        let Some((read, write)) = pipe() else {
            log::warn!("signals: no pipe, a termination signal ends rox without its quit");
            return;
        };
        WAKE.store(write, Ordering::Relaxed);

        let caught: Vec<c_int> = SIGNALS
            .into_iter()
            .filter(|&signal| catch(signal))
            .collect();

        let (tx, rx) = async_channel::bounded::<()>(1);
        let waiter = {
            let caught = caught.clone();
            std::thread::Builder::new()
                .name("rox-signals".into())
                .spawn(move || wait(read, &caught, tx))
        };
        if let Err(e) = waiter {
            log::warn!("signals: no waiter thread, termination signals keep their default: {e}");
            caught.iter().for_each(|&signal| restore(signal));
            return;
        }

        cx.spawn(async move |cx| {
            if rx.recv().await.is_err() {
                return;
            }

            let _ = cx.update(|cx| {
                log::info!("signals: quitting on a termination signal");
                cx.dispatch_action(&Quit);
            });
        })
        .detach();
    }

    extern "C" fn on_signal(_: c_int) {
        let fd = WAKE.load(Ordering::Relaxed);
        if fd < 0 {
            return;
        }

        let byte = 1u8;
        // SAFETY: errno's slot is this thread's own and always valid, and
        // write(2) is async-signal-safe. The byte lives in this frame. errno
        // goes back as it was so the code this interrupted doesn't read ours.
        unsafe {
            let errno = errno_slot();
            let saved = *errno;
            libc::write(fd, (&raw const byte).cast(), 1);
            *errno = saved;
        }
    }

    #[cfg(target_os = "macos")]
    fn errno_slot() -> *mut c_int {
        // SAFETY: returns the calling thread's errno, valid for its life.
        unsafe { libc::__error() }
    }

    #[cfg(not(target_os = "macos"))]
    fn errno_slot() -> *mut c_int {
        // SAFETY: returns the calling thread's errno, valid for its life.
        unsafe { libc::__errno_location() }
    }

    /// A close-on-exec pipe, so no child inherits it, with a write end that
    /// never blocks the handler.
    fn pipe() -> Option<(c_int, c_int)> {
        let mut fds = [0 as c_int; 2];
        // SAFETY: pipe(2) writes two descriptors into the array it's given.
        if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
            return None;
        }

        // SAFETY: fcntl on descriptors this function just opened and owns.
        unsafe {
            for fd in fds {
                libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
            }
            let flags = libc::fcntl(fds[1], libc::F_GETFL);
            libc::fcntl(fds[1], libc::F_SETFL, flags | libc::O_NONBLOCK);
        }

        Some((fds[0], fds[1]))
    }

    /// Point `signal` at the handler unless it was ignored at launch. True
    /// when it now runs the handler.
    fn catch(signal: c_int) -> bool {
        // SAFETY: sigaction(2) with a zeroed struct for the query, which it
        // only writes, then a filled one it only reads. A zeroed sigaction
        // is a valid value: no flags, empty mask, SIG_DFL.
        unsafe {
            let mut previous: libc::sigaction = std::mem::zeroed();
            if libc::sigaction(signal, std::ptr::null(), &mut previous) != 0 {
                return false;
            }
            if previous.sa_sigaction == libc::SIG_IGN {
                return false;
            }

            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = on_signal as extern "C" fn(c_int) as libc::sighandler_t;
            // RESETHAND: the kernel puts the default back before the handler
            // runs, so a second signal of the same kind kills outright.
            action.sa_flags = libc::SA_RESETHAND | libc::SA_RESTART;
            libc::sigemptyset(&mut action.sa_mask);
            libc::sigaction(signal, &action, std::ptr::null_mut()) == 0
        }
    }

    fn restore(signal: c_int) {
        // SAFETY: as in `catch`, a zeroed sigaction is SIG_DFL with no flags.
        unsafe {
            let action: libc::sigaction = std::mem::zeroed();
            libc::sigaction(signal, &action, std::ptr::null_mut());
        }
    }

    fn wait(read: c_int, caught: &[c_int], tx: async_channel::Sender<()>) {
        let mut byte = 0u8;
        loop {
            // SAFETY: reads at most one byte into a local only this call uses.
            let n = unsafe { libc::read(read, (&raw mut byte).cast(), 1) };
            if n == 1 {
                break;
            }
            if n < 0 && std::io::Error::last_os_error().kind() == ErrorKind::Interrupted {
                continue;
            }
            return;
        }

        // The signal that came reset itself; the other two follow, so any
        // second signal ends a quit that hangs.
        caught.iter().for_each(|&signal| restore(signal));
        let _ = tx.send_blocking(());
    }
}
