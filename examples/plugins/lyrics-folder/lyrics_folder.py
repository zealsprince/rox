"""Lyrics Folder: an example rox provider plugin, written to be copied.

rox starts this file as a subprocess and speaks newline-delimited JSON-RPC 2.0
with it: one request per line on stdin, one answer per line on stdout. stderr
is free text that lands in rox's log. The protocol is written down in
README_PLUGINS.md in the rox repository.

It's no source: it has no tracks of its own. It declares providers.lyrics,
so once the user switches Lyrics on for it, rox asks provider.lyrics for any
track in the library by its tags, and this answers from the .lrc files under
the folder its config names. It reads nothing outside that folder and writes
nothing anywhere.

Only the standard library is used, so there's nothing to install.
"""

import concurrent.futures
import json
import os
import re
import sys
import threading

API = 1

# rox refuses an answer over either cap, so the plugin keeps under them.
MAX_CANDIDATES = 50
MAX_LYRICS = 256 * 1024
MAX_STRING = 4096

TAG = re.compile(r"^\[(ar|ti|al):(.*)\]\s*$", re.IGNORECASE)
TIMED = re.compile(r"^\[\d+:\d\d")


class Failure(Exception):
    """An error answer: a JSON-RPC code and a message rox shows or logs."""

    def __init__(self, code, message):
        super().__init__(message)
        self.code = code


state = {"folder": None}
out_lock = threading.Lock()


def send(frame):
    # Writes come from several threads, so each whole line goes out under a
    # lock. Bytes, so Windows doesn't turn "\n" into "\r\n".
    line = (json.dumps(frame, separators=(",", ":")) + "\n").encode("utf-8")
    with out_lock:
        sys.stdout.buffer.write(line)
        sys.stdout.buffer.flush()


def log(text):
    sys.stderr.write(text + "\n")
    sys.stderr.flush()


def fold(text):
    """Lowercase, with every run of anything but letters and digits as one
    space, the way rox folds names before it compares them."""
    out = []
    last_space = True
    for ch in text:
        if ch.isalnum():
            out.append(ch.lower())
            last_space = False
        elif not last_space:
            out.append(" ")
            last_space = True
    return "".join(out).rstrip(" ")


def hello(params):
    config = params.get("config") or {}
    folder = config.get("folder")

    if isinstance(folder, str) and folder.strip():
        folder = os.path.expanduser(folder.strip())
        if os.path.isabs(folder):
            state["folder"] = os.path.realpath(folder)
        else:
            log(f"the folder has to be a full path, not {folder!r}")

    log(f"hello from rox, api {params.get('api')}, searching {state['folder']}")
    return {"name": "Lyrics Folder", "version": "1.0.0", "api": API}


def sheets(root):
    """Every .lrc file under root, subfolders included."""
    for folder, _, files in os.walk(root):
        for name in sorted(files):
            if not name.lower().endswith(".lrc"):
                continue

            path = os.path.join(folder, name)
            # A link inside the folder can point anywhere; only files that
            # really live under it are read.
            if inside(os.path.realpath(path), root):
                yield path


def inside(path, root):
    try:
        return os.path.commonpath([path, root]) == root
    except ValueError:
        # Windows: a path on another drive.
        return False


def read(path):
    """The file's text, its tags, and the stem's artist and title."""
    with open(path, encoding="utf-8-sig", errors="replace") as f:
        text = f.read()

    tags = {}
    for line in text.splitlines():
        found = TAG.match(line.strip())
        if found:
            tags.setdefault(found.group(1).lower(), found.group(2).strip())

    stem = os.path.splitext(os.path.basename(path))[0]
    artist, dash, title = stem.partition(" - ")
    named = (artist, title) if dash else (None, None)

    return text, tags, named


def candidate(text, artist, title, album, tagged):
    found = {
        "artist": artist,
        "title": title,
        "album": album,
        "text": text,
        "synced": any(TIMED.match(line.strip()) for line in text.splitlines()),
    }
    # A match by the file's own tags is as sure as this plugin gets. One by
    # its name leaves the ranking to rox's own comparison.
    if tagged:
        found["confidence"] = 1.0
    return found


def lyrics(params):
    root = state["folder"]
    want = (fold(str(params.get("artist") or "")), fold(str(params.get("title") or "")))
    if root is None or not os.path.isdir(root) or not all(want):
        return {"candidates": []}

    found = []
    for path in sheets(root):
        try:
            text, tags, (stem_artist, stem_title) = read(path)
        except OSError as e:
            log(f"can't read {path}: {e}")
            continue

        if not text.strip() or len(text.encode("utf-8")) > MAX_LYRICS:
            continue

        tagged = bool(tags.get("ar")) and bool(tags.get("ti"))
        artist = tags["ar"] if tagged else stem_artist
        title = tags["ti"] if tagged else stem_title
        if artist is None or (fold(artist), fold(title)) != want:
            continue

        album = tags.get("al", "")
        if any(len(field.encode("utf-8")) > MAX_STRING for field in (artist, title, album)):
            continue

        found.append(candidate(text, artist, title, album, tagged))
        if len(found) == MAX_CANDIDATES:
            break

    return {"candidates": found}


METHODS = {
    "hello": hello,
    "provider.lyrics": lyrics,
}


def answer(rid, method, params):
    handler = METHODS.get(method)
    try:
        if handler is None:
            raise Failure(-32601, f"method not found: {method}")
        send({"jsonrpc": "2.0", "id": rid, "result": handler(params)})
    except Failure as e:
        send({"jsonrpc": "2.0", "id": rid, "error": {"code": e.code, "message": str(e)}})
    except Exception as e:
        send({"jsonrpc": "2.0", "id": rid, "error": {"code": -32000, "message": f"{type(e).__name__}: {e}"}})


def main():
    pool = concurrent.futures.ThreadPoolExecutor(max_workers=4)

    # Ends when stdin closes, whether rox shut down cleanly or not.
    for raw in sys.stdin.buffer:
        if not raw.strip():
            continue

        try:
            frame = json.loads(raw)
        except ValueError as e:
            log(f"a line that isn't JSON: {e}")
            continue

        rid, method, params = frame.get("id"), frame.get("method"), frame.get("params") or {}

        if method == "shutdown":
            send({"jsonrpc": "2.0", "id": rid, "result": None})
            break

        pool.submit(answer, rid, method, params)

    pool.shutdown(wait=False, cancel_futures=True)


if __name__ == "__main__":
    main()
