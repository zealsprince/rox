"""A renderer that tries to leave the sandbox (ADR 38), for the test in
tests/sandbox.rs and for a run in a real rox.

It reads what to try from targets.json in its own folder, tries each, and
prints one line per try: `probe <name> <reached|blocked> <detail>`. It never
draws. Started by rox, it reads `hello`, prints to stderr, which rox logs,
and waits for stdin to close. With PROBE_OUT=stdout it prints there and
exits, which is how the test reads it.

Every network try targets 127.0.0.1 only.
"""

import ctypes
import json
import mmap
import os
import signal
import socket
import sys

OUT = sys.stdout if os.environ.get("PROBE_OUT") == "stdout" else sys.stderr
RENDERER = "ROX_RENDER_API" in os.environ and OUT is sys.stderr


def say(name, reached, detail=""):
    word = "reached" if reached else "blocked"
    detail = str(detail).replace("\n", " ")[:160]
    print(f"probe {name} {word} {detail}", file=OUT, flush=True)


def attempt(name, action):
    try:
        say(name, True, action())
    except Exception as e:  # what blocked it is the detail
        say(name, False, f"{type(e).__name__}: {e}")


def read_file(path):
    with open(path, "rb") as f:
        return f"read {len(f.read(64))} bytes"


def list_dir(path):
    return f"listed {len(os.listdir(path))} entries"


def write_file(path):
    with open(path, "w") as f:
        f.write("probe\n")
    os.remove(path)
    return f"wrote {path}"


def tcp(port):
    with socket.create_connection(("127.0.0.1", port), timeout=2) as s:
        return f"connected to {s.getpeername()}"


def udp(port):
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        s.sendto(b"probe", ("127.0.0.1", port))
        return "sent a datagram"
    finally:
        s.close()


def unix(path):
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        s.settimeout(2)
        s.connect(path)
        return f"connected to {path!r}"
    finally:
        s.close()


def signal_to(pid, sig):
    os.kill(pid, sig)
    return f"signal {int(sig)} to {pid} delivered"


def ptrace_attach(pid):
    libc = ctypes.CDLL(None, use_errno=True)
    libc.ptrace.argtypes = [ctypes.c_long, ctypes.c_long, ctypes.c_void_p, ctypes.c_void_p]
    if libc.ptrace(16, pid, None, None) != 0:  # PTRACE_ATTACH
        raise OSError(ctypes.get_errno(), os.strerror(ctypes.get_errno()))
    libc.ptrace(17, pid, None, None)  # PTRACE_DETACH
    return f"attached to {pid}"


class IoVec(ctypes.Structure):
    _fields_ = [("base", ctypes.c_void_p), ("len", ctypes.c_size_t)]


def vm_read(pid):
    libc = ctypes.CDLL(None, use_errno=True)
    buf = ctypes.create_string_buffer(8)
    local = IoVec(ctypes.cast(buf, ctypes.c_void_p), 8)
    # Any address: refused before it's looked at, or EFAULT if it's let through.
    remote = IoVec(0x400000, 8)
    got = libc.process_vm_readv(pid, ctypes.byref(local), 1, ctypes.byref(remote), 1, 0)
    if got < 0 and ctypes.get_errno() != 14:  # EFAULT means it got that far
        raise OSError(ctypes.get_errno(), os.strerror(ctypes.get_errno()))
    return f"process_vm_readv answered {got}, errno {ctypes.get_errno()}"


def lent_fds():
    names = ["ROX_FRAMES_FD", "ROX_COVER_FD", "ROX_TEXTURE_FD", "ROX_SPECTRUM_FD"]
    seen = []
    for name in names:
        fd = os.environ.get(name)
        if fd is None:
            continue
        fd = int(fd)
        size = os.fstat(fd).st_size
        # The texture is GPU memory: mapping it isn't how it's used.
        if name != "ROX_TEXTURE_FD" and size > 0:
            with mmap.mmap(fd, min(size, 4096), mmap.MAP_SHARED, mmap.PROT_READ) as m:
                seen.append(f"{name}={fd} {size} bytes {bytes(m[:4])!r}")
        else:
            seen.append(f"{name}={fd} {size} bytes")
    if not seen:
        raise RuntimeError("no lent fd named")
    return ", ".join(seen)


def main():
    if RENDERER:
        hello = sys.stdin.readline()
        print(f"probe hello {hello.strip()[:120]}", file=OUT, flush=True)

    here = os.path.dirname(os.path.abspath(__file__))
    with open(os.path.join(here, "targets.json")) as f:
        t = json.load(f)

    rox = int(t.get("rox_pid") or os.getppid())
    cache = os.environ.get("ROX_CACHE_DIR", "")

    print(f"probe start pid {os.getpid()} rox {rox} cache {cache}", file=OUT, flush=True)

    # Must be blocked.
    attempt("home-file", lambda: read_file(t["home_file"]))
    attempt("home-list", lambda: list_dir(t["home"]))
    attempt("data-dir-file", lambda: read_file(t["data_file"]))
    attempt("data-dir-list", lambda: list_dir(t["data_dir"]))
    attempt("other-package", lambda: list_dir(t["other_package"]))
    attempt("tcp-localhost", lambda: tcp(t["tcp_port"]))
    attempt("udp-socket", lambda: udp(t["udp_port"]))
    attempt("session-bus", lambda: unix(t["bus"]))
    attempt("wayland", lambda: unix(t["wayland"]))
    attempt("abstract-unix", lambda: unix("\0" + t["abstract"]))
    attempt("write-package", lambda: write_file(os.path.join(here, "written")))
    attempt("write-outside", lambda: write_file(t["write_outside"]))
    attempt("signal-rox-0", lambda: signal_to(rox, 0))
    attempt("signal-rox-cont", lambda: signal_to(rox, signal.SIGCONT))
    attempt("proc-environ", lambda: read_file(f"/proc/{rox}/environ"))
    attempt("proc-mem", lambda: read_file(f"/proc/{rox}/mem"))
    attempt("proc-cmdline", lambda: read_file(f"/proc/{rox}/cmdline"))
    attempt("ptrace-rox", lambda: ptrace_attach(rox))
    attempt("vm-read-rox", lambda: vm_read(rox))
    attempt("proc-self", lambda: read_file("/proc/self/status"))
    attempt("dev-shm", lambda: list_dir("/dev/shm"))
    attempt("run-user", lambda: list_dir(f"/run/user/{os.getuid()}"))
    attempt("etc-passwd", lambda: read_file("/etc/passwd"))
    attempt("tmp-list", lambda: list_dir("/tmp"))

    # Must be reached.
    attempt("own-folder", lambda: read_file(os.path.join(here, "plugin.json")))
    attempt("own-list", lambda: list_dir(here))
    attempt("cache-write", lambda: write_file(os.path.join(cache, "probe-wrote")))
    attempt("lent-fds", lent_fds)

    # Only with the asks granted.
    if "library_file" in t:
        attempt("library-file", lambda: read_file(t["library_file"]))
        attempt("beside-library", lambda: read_file(t["beside_file"]))

    # Only with an interpreter installed under home.
    if "install_file" in t:
        attempt("install-file", lambda: read_file(t["install_file"]))
        attempt("beside-install", lambda: read_file(t["beside_install"]))

    # Learning a path exists isn't stopped.
    attempt("home-stat", lambda: f"size {os.stat(t['home_file']).st_size}")

    print("probe done", file=OUT, flush=True)

    if RENDERER:
        # Exits when rox closes stdin.
        for _ in sys.stdin:
            pass


main()
