//! Spawning a plugin and killing it with everything it started. A plugin
//! runs with the user's permissions, so the one thing the host owes the user
//! is that switching it off, or quitting rox, leaves nothing of it running.
//!
//! Unix puts the plugin in its own process group and signals the group.
//! Windows assigns it to a job object that kills everything in it when the
//! handle closes; a grandchild spawned between the spawn and the assignment
//! escapes, and the contract accepts that.

use std::io::{self, BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use crate::search;

/// Without it every plugin start pops a console window.
#[cfg(windows)]
pub(crate) const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Longest stderr line kept; the rest of the line is dropped.
const STDERR_LINE: usize = 4096;

pub struct Process {
    child: Child,
    #[cfg(windows)]
    job: Option<job::Job>,
    /// The extra handle's value inside the child, on Windows only.
    handle: Option<u64>,
}

pub struct Pipes {
    pub stdin: ChildStdin,
    pub stdout: ChildStdout,
}

/// A handle rox lends a child, borrowed for the spawn.
#[cfg(unix)]
pub type Lent<'a> = std::os::fd::BorrowedFd<'a>;

/// A handle rox lends a child, borrowed for the spawn, with the rights the
/// child's copy gets. Never more than the child needs: a section lent for
/// frames is mapped and nothing else.
#[cfg(windows)]
#[derive(Clone, Copy)]
pub struct Lent<'a> {
    pub handle: std::os::windows::io::BorrowedHandle<'a>,
    pub access: u32,
}

/// What a child gets beside its three pipes.
pub enum Extra<'a> {
    /// Nothing, which is what a plugin gets: on Unix every fd past 2 is
    /// closed at exec.
    None,
    /// One handle: fd 3 on Unix, with every other fd closed at exec. On
    /// Windows a copy with the lent access is made in the child once it
    /// runs, and [`Process::handle`] says its value.
    Handle(Lent<'a>),
}

/// Starts `command` in the plugin folder with its pipes attached. The
/// plugin inherits rox's environment (HOME for its programs' caches) with
/// PATH replaced by the plugin's search path, so what it runs resolves the
/// way the Plugins page reported it, plus three variables for Python.
pub fn spawn(id: &str, command: Command, dir: &Path) -> Result<(Process, Pipes), String> {
    spawn_with("plugin", id, command, dir, Extra::None)
}

/// [`spawn`], lending the child one more handle. `kind` leads the child's
/// log lines, its stderr included: `plugin` or `renderer`.
pub fn spawn_with(
    kind: &'static str,
    id: &str,
    mut command: Command,
    dir: &Path,
    extra: Extra,
) -> Result<(Process, Pipes), String> {
    command
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("PATH", search::search_path(Some(dir)))
        // A Python plugin's first import would write `__pycache__` into its
        // own folder and change its own hash.
        .env("PYTHONDONTWRITEBYTECODE", "1")
        // A piped stdout is block-buffered, so answers would sit unsent.
        .env("PYTHONUNBUFFERED", "1")
        // Windows reads piped text in the ANSI code page, not UTF-8.
        .env("PYTHONUTF8", "1");

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    // Held until the spawn returns: the child dups it onto fd 3.
    #[cfg(unix)]
    let _lent = match &extra {
        Extra::None => {
            unix::inherit_pipes_only(&mut command);
            None
        }
        Extra::Handle(fd) => Some(unix::lend(&mut command, *fd)?),
    };

    let mut child = command
        .spawn()
        .map_err(|e| format!("could not start: {e}"))?;

    #[cfg(windows)]
    let job = match job::Job::adopt(&child) {
        Ok(job) => Some(job),
        Err(e) => {
            log::warn!("{kind} {id}: no job object, its children may outlive it: {e}");
            None
        }
    };

    #[cfg(windows)]
    let handle = match &extra {
        Extra::None => None,
        Extra::Handle(lent) => match job::lend(&child, *lent) {
            Ok(value) => Some(value),
            Err(e) => {
                drop(job);
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("could not hand over the handle: {e}"));
            }
        },
    };
    #[cfg(unix)]
    let handle = None;

    let (Some(stdin), Some(stdout), Some(stderr)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        let _ = child.kill();
        return Err("could not attach to the plugin's pipes".into());
    };

    let tag = format!("{kind} {id}");
    let drained = std::thread::Builder::new()
        .name(format!("{kind}-{id}-stderr"))
        .spawn(move || drain_stderr(&tag, stderr));
    if let Err(e) = drained {
        log::warn!("{kind} {id}: stderr goes unread: {e}");
    }

    let process = Process {
        child,
        #[cfg(windows)]
        job,
        handle,
    };

    Ok((process, Pipes { stdin, stdout }))
}

#[cfg(unix)]
mod unix {
    use std::io;
    use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};
    use std::os::unix::process::CommandExt;
    use std::process::Command;

    /// The fd a lent handle arrives as.
    pub const FD: RawFd = 3;

    /// Sets the child up to get `fd` as fd 3 and nothing past it. Answers a
    /// copy of `fd` numbered above 3, to keep open until the spawn returns.
    ///
    /// The copy is what the child dups from. A handle that already sits at
    /// fd 3 would otherwise be the target of its own dup, and one at 0 to 2
    /// is overwritten by the pipes before `pre_exec` runs.
    pub fn lend(command: &mut Command, fd: BorrowedFd) -> Result<OwnedFd, String> {
        // SAFETY: fcntl on a borrowed open fd; the result is a fresh fd this
        // function owns.
        let copy = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, FD + 1) };
        if copy < 0 {
            return Err(format!(
                "could not copy the handle: {}",
                io::Error::last_os_error()
            ));
        }
        // SAFETY: `copy` was just opened and nothing else holds it.
        let copy = unsafe { OwnedFd::from_raw_fd(copy) };

        let source = copy.as_raw_fd();
        let limit = open_max();

        // SAFETY: the closure runs in the forked child before exec and only
        // makes async-signal-safe calls: dup2, fcntl and close_range.
        unsafe {
            command.pre_exec(move || {
                // dup2 leaves the new fd without close-on-exec.
                if libc::dup2(source, FD) < 0 {
                    return Err(io::Error::last_os_error());
                }

                close_from(FD + 1, limit);
                Ok(())
            });
        }

        Ok(copy)
    }

    /// Sets the child up to keep its three pipes and nothing past them.
    pub fn inherit_pipes_only(command: &mut Command) {
        let limit = open_max();

        // SAFETY: the closure runs in the forked child before exec and only
        // makes async-signal-safe calls: fcntl and close_range.
        unsafe {
            command.pre_exec(move || {
                close_from(3, limit);
                Ok(())
            });
        }
    }

    /// rox holds its database, sockets and audio devices open, and a library
    /// rox links may open an fd without close-on-exec. Every fd from `first`
    /// up is marked so it closes at exec. Runs in the forked child, so only
    /// async-signal-safe calls.
    unsafe fn close_from(first: RawFd, limit: RawFd) {
        // SAFETY: plain fd calls in the child; see the callers.
        unsafe {
            #[cfg(target_os = "linux")]
            {
                let marked = libc::syscall(
                    libc::SYS_close_range,
                    first as libc::c_uint,
                    libc::c_uint::MAX,
                    libc::CLOSE_RANGE_CLOEXEC,
                );
                if marked == 0 {
                    return;
                }
            }

            // Kernels before 5.11, and every other Unix.
            for fd in first..limit {
                let flags = libc::fcntl(fd, libc::F_GETFD);
                if flags >= 0 && flags & libc::FD_CLOEXEC == 0 {
                    libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
                }
            }
        }
    }

    /// The highest fd number the walk in [`close_from`] tries, read here
    /// since getrlimit isn't on the async-signal-safe list.
    fn open_max() -> RawFd {
        let mut limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };

        // SAFETY: getrlimit writes the struct it's handed.
        let read = unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) };
        match read {
            0 => limit.rlim_cur.min(1 << 20) as RawFd,
            _ => 1024,
        }
    }
}

/// Makes rox's end of a child's stdin refuse a write the pipe has no room
/// for, rather than wait on a child that stopped reading. A line of at most
/// `PIPE_BUF` bytes then goes whole or not at all.
#[cfg(unix)]
pub fn never_block(stdin: &ChildStdin) -> io::Result<()> {
    use std::os::fd::AsRawFd;

    let fd = stdin.as_raw_fd();

    // SAFETY: fcntl on an fd `stdin` holds open for the call.
    let set = unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        flags >= 0 && libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) == 0
    };
    if !set {
        return Err(io::Error::last_os_error());
    }

    Ok(())
}

/// Windows leaves the pipe blocking: `PIPE_NOWAIT` is a legacy mode on an
/// anonymous pipe, and nothing here has been compiled for Windows to try it.
#[cfg(windows)]
pub fn never_block(_stdin: &ChildStdin) -> io::Result<()> {
    Ok(())
}

/// `tag` is the child's kind and id, as its lines lead with them.
fn drain_stderr(tag: &str, stderr: impl Read) {
    let mut reader = BufReader::new(stderr);
    let mut line = Vec::new();

    loop {
        match read_line(&mut reader, &mut line, STDERR_LINE) {
            Ok(Line::End) | Err(_) => return,

            Ok(Line::Whole | Line::Clipped) => {
                let text = String::from_utf8_lossy(&line);
                let text = text.trim_end();
                if !text.is_empty() {
                    log::info!("{tag}: {text}");
                }
            }
        }
    }
}

pub enum Line {
    Whole,
    /// Longer than the cap; `buf` holds the head and the rest was skipped.
    Clipped,
    End,
}

/// One line into `buf`, newline stripped, never holding more than `cap`
/// bytes however long the line runs.
pub fn read_line(reader: &mut impl BufRead, buf: &mut Vec<u8>, cap: usize) -> io::Result<Line> {
    buf.clear();
    let mut clipped = false;

    loop {
        let available = match reader.fill_buf() {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };

        if available.is_empty() {
            return Ok(match (buf.is_empty() && !clipped, clipped) {
                (true, _) => Line::End,
                (false, true) => Line::Clipped,
                (false, false) => Line::Whole,
            });
        }

        let (take, newline) = match available.iter().position(|b| *b == b'\n') {
            Some(at) => (at, true),
            None => (available.len(), false),
        };

        let room = cap.saturating_sub(buf.len());
        if take > room {
            clipped = true;
        }
        buf.extend_from_slice(&available[..take.min(room)]);
        reader.consume(take + usize::from(newline));

        if newline {
            return Ok(match clipped {
                true => Line::Clipped,
                false => Line::Whole,
            });
        }
    }
}

impl Process {
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// The lent handle's value in the child on Windows. None on Unix, where
    /// it is always fd 3, and when nothing was lent.
    pub fn handle(&self) -> Option<u64> {
        self.handle
    }

    pub fn exited(&mut self) -> Option<ExitStatus> {
        self.child.try_wait().ok().flatten()
    }

    /// True once it's gone, false if it's still running at the deadline.
    pub fn wait_for(&mut self, within: Duration) -> bool {
        let deadline = Instant::now() + within;

        loop {
            if self.exited().is_some() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }

            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// The plugin and everything it started. Safe to call twice.
    pub fn kill(&mut self) {
        #[cfg(unix)]
        {
            // `process_group(0)` made the plugin its group's leader, so its pid
            // is the group id. A child that re-grouped itself escapes, as it
            // would from any shell.
            let group = self.child.id() as libc::pid_t;

            // SAFETY: kill(2) on our own child's group; no memory is shared.
            unsafe {
                libc::kill(-group, libc::SIGKILL);
            }
        }

        #[cfg(windows)]
        {
            // Closing the last handle kills every process in the job.
            self.job.take();
        }

        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        self.kill();
    }
}

#[cfg(windows)]
mod job {
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };

    pub struct Job(HANDLE);

    // SAFETY: a job handle is a kernel object reference, usable from any thread.
    unsafe impl Send for Job {}

    impl Job {
        pub fn adopt(child: &Child) -> Result<Job, String> {
            // SAFETY: plain Win32 calls on handles this function owns or borrows
            // from `child` for the duration of the call.
            unsafe {
                let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if handle.is_null() {
                    return Err(std::io::Error::last_os_error().to_string());
                }
                let job = Job(handle);

                let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let set = SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    &limits as *const _ as *const core::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                if set == 0 {
                    return Err(std::io::Error::last_os_error().to_string());
                }

                if AssignProcessToJobObject(job.0, child.as_raw_handle() as HANDLE) == 0 {
                    return Err(std::io::Error::last_os_error().to_string());
                }

                Ok(job)
            }
        }
    }

    /// Copies `lent` into the running child with the lent access only, and
    /// answers the copy's value there. Nothing is inheritable: the copy
    /// lands in that one process.
    pub fn lend(child: &Child, lent: super::Lent) -> Result<u64, String> {
        use windows_sys::Win32::Foundation::DuplicateHandle;
        use windows_sys::Win32::System::Threading::GetCurrentProcess;

        let mut copy: HANDLE = std::ptr::null_mut();

        // SAFETY: both process handles are live for the call, `lent` is a
        // borrowed open handle, and `copy` is written by the call. No
        // options: the access is the one asked for, not the source's.
        let done = unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                lent.handle.as_raw_handle() as HANDLE,
                child.as_raw_handle() as HANDLE,
                &mut copy,
                lent.access,
                0,
                0,
            )
        };
        if done == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }

        Ok(copy as usize as u64)
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: the handle came from CreateJobObjectW and is closed once.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(input: &[u8], cap: usize) -> Vec<(String, bool)> {
        let mut reader = BufReader::with_capacity(4, input);
        let mut buf = Vec::new();
        let mut out = Vec::new();

        loop {
            match read_line(&mut reader, &mut buf, cap).unwrap() {
                Line::End => return out,
                Line::Whole => out.push((String::from_utf8_lossy(&buf).into_owned(), false)),
                Line::Clipped => out.push((String::from_utf8_lossy(&buf).into_owned(), true)),
            }
        }
    }

    #[test]
    fn lines_split_on_newlines_across_small_buffers() {
        assert_eq!(
            lines(b"one\ntwo three\n\nlast", 64),
            vec![
                ("one".into(), false),
                ("two three".into(), false),
                ("".into(), false),
                ("last".into(), false),
            ]
        );
    }

    #[test]
    fn a_long_line_is_clipped_and_the_next_one_still_reads() {
        assert_eq!(
            lines(b"abcdefghij\nok\n", 4),
            vec![("abcd".into(), true), ("ok".into(), false)]
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_plugin_gets_the_python_variables_and_its_bin_first_on_path() {
        let dir = std::env::temp_dir().join(format!("rox-plugins-env-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg(r#"printf '%s\n%s\n%s\n' "$PYTHONUNBUFFERED" "$PYTHONUTF8" "$PATH""#);
        let (mut process, pipes) = spawn("env", command, &dir).unwrap();

        let mut out = String::new();
        let mut stdout = pipes.stdout;
        stdout.read_to_string(&mut out).unwrap();
        process.wait_for(Duration::from_secs(5));
        let _ = std::fs::remove_dir_all(&dir);

        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[..2], ["1", "1"], "{out}");

        let first = std::env::split_paths(lines[2]).next();
        assert_eq!(first, Some(dir.join("bin")), "{out}");
    }

    /// Runs `script` under `sh` with `file` lent, and answers its stdout.
    #[cfg(unix)]
    fn lend_to(tag: &str, file: &std::fs::File, script: &str) -> String {
        use std::os::fd::AsFd;

        let dir =
            std::env::temp_dir().join(format!("rox-plugins-lend-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let mut command = Command::new("sh");
        command.arg("-c").arg(script);
        let (mut process, pipes) =
            spawn_with("renderer", tag, command, &dir, Extra::Handle(file.as_fd())).unwrap();
        assert_eq!(process.handle(), None, "Unix names no handle: it's fd 3");

        let mut out = String::new();
        let mut stdout = pipes.stdout;
        stdout.read_to_string(&mut out).unwrap();
        process.wait_for(Duration::from_secs(5));
        let _ = std::fs::remove_dir_all(&dir);

        out
    }

    #[cfg(unix)]
    fn scratch_file(tag: &str, text: &str) -> (std::path::PathBuf, std::fs::File) {
        let path = std::env::temp_dir().join(format!("rox-plugins-{tag}-{}", std::process::id()));
        std::fs::write(&path, text).unwrap();
        let file = std::fs::File::open(&path).unwrap();

        (path, file)
    }

    #[cfg(unix)]
    #[test]
    fn the_extra_handle_arrives_as_fd_three() {
        let (path, file) = scratch_file("fd-three", "the frames\n");

        let out = lend_to("fd-three", &file, "cat <&3");
        let _ = std::fs::remove_file(&path);
        assert_eq!(out, "the frames\n");

        // Lent, not given: rox's own fd still reads.
        use std::io::{Seek, SeekFrom};
        let mut file = file;
        file.seek(SeekFrom::Start(0)).unwrap();
        let mut again = String::new();
        file.read_to_string(&mut again).unwrap();
        assert_eq!(again, "the frames\n");
    }

    /// An fd without close-on-exec, as a C dependency's open(2) leaves it,
    /// numbered well above 3 where a lent handle's dup2 can't hide it.
    #[cfg(unix)]
    fn careless_fd(tag: &str) -> (std::path::PathBuf, std::os::fd::OwnedFd) {
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

        let (path, file) = scratch_file(tag, "secret\n");
        // SAFETY: F_DUPFD makes a fresh fd, close-on-exec clear, owned here.
        let held = unsafe {
            let raw = libc::fcntl(file.as_raw_fd(), libc::F_DUPFD, 64);
            assert!(raw >= 64);
            OwnedFd::from_raw_fd(raw)
        };

        (path, held)
    }

    /// A script listing the child's open fds up to `top`.
    #[cfg(unix)]
    fn fd_probe(top: i32) -> String {
        format!(
            r#"n=0; while [ $n -le {top} ]; do [ -e /dev/fd/$n ] && printf '%s ' $n; n=$((n+1)); done"#
        )
    }

    #[cfg(unix)]
    #[test]
    fn no_other_fd_is_inherited() {
        use std::os::fd::AsRawFd;

        let (path, held) = careless_fd("no-leak");
        let raw = held.as_raw_fd();
        let (lent_path, lent) = scratch_file("no-leak-lent", "frames\n");

        let top = raw.max(lent.as_raw_fd()) + 16;
        let out = lend_to("no-leak", &lent, &fd_probe(top));
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&lent_path);

        let open: Vec<i32> = out.split_whitespace().map(|n| n.parse().unwrap()).collect();
        // Seeing 3 is what shows the probe finds an open fd at all.
        assert_eq!(open, [0, 1, 2, 3], "the child holds {out:?}; {raw} leaked");

        drop(held);
    }

    #[cfg(unix)]
    #[test]
    fn a_plugin_inherits_only_its_pipes() {
        use std::os::fd::AsRawFd;

        let (path, held) = careless_fd("plain-no-leak");
        let raw = held.as_raw_fd();

        let dir = std::env::temp_dir().join(format!("rox-plugins-plain-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let mut command = Command::new("sh");
        command.arg("-c").arg(fd_probe(raw + 16));
        let (mut process, pipes) = spawn("plain-no-leak", command, &dir).unwrap();

        let mut out = String::new();
        let mut stdout = pipes.stdout;
        stdout.read_to_string(&mut out).unwrap();
        process.wait_for(Duration::from_secs(5));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_file(&path);

        let open: Vec<i32> = out.split_whitespace().map(|n| n.parse().unwrap()).collect();
        assert_eq!(open, [0, 1, 2], "the plugin holds {out:?}; {raw} leaked");

        drop(held);
    }
}
