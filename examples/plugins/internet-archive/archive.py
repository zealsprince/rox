"""Internet Archive: an example rox source plugin that uses every optional part
of the protocol.

It browses the Internet Archive's netlabels collection, thousands of releases
their labels put out under Creative Commons, and plays them. Only releases
whose licence is Creative Commons or public domain are listed, so everything it
shows is free to stream and to show. The protocol is written down in
README_PLUGINS.md in the rox repository, and the tones example beside this one
is the smaller place to start.

What this one adds over tones:

- A home that rox lists under the roots (`home`), made of headed shelves of
  covers (`sections`, `tiles`) and rows.
- Release and label covers (`node-kind`, `node-art`, `source.cover`).
- Columns the tags don't carry, a download count and a year (`fields`), and
  orders the Archive applies (`views`).
- A line over the roots with a link (`notice`, `notice-link`).
- A radio (`source.radio`) and links to each item's page (`source.link`).
- An action, Download Original, that saves a song's or a release's original
  upload as a job rox polls for progress (`source.action`, `source.job`,
  `source.cancel`).
- Audio streamed from a web server with range requests on a kept-alive
  connection, so a seek in rox is a seek on the server.

Every optional part is sent only when `hello` listed it, so an older rox gets
plain rows and nothing it would refuse. Only the standard library is used.
"""

import base64
import concurrent.futures
import functools
import hashlib
import http.client
import json
import os
import re
import sys
import threading
import urllib.parse
import urllib.request

API = 1
VERSION = "1.0.0"

SEARCH_URL = "https://archive.org/advancedsearch.php"
METADATA_URL = "https://archive.org/metadata/"
DETAILS_URL = "https://archive.org/details/"
DOWNLOAD_URL = "https://archive.org/download/"
THUMB_URL = "https://archive.org/services/img/"

# The Archive asks clients to say who they are.
USER_AGENT = f"rox-internet-archive-example/{VERSION} (+https://github.com/zealsprince/rox)"
HTTP_TIMEOUT = 12

# Only freely licensed releases, whatever else a query asks for.
FREE = "licenseurl:(*creativecommons* OR *publicdomain*)"
RELEASES = f"collection:netlabels AND mediatype:audio AND {FREE}"
LABELS = "collection:netlabels AND mediatype:collection"

PAGE = 50
SHELF = 12
STARTERS = 6

# A radio batch reads this many releases at once, and answers with what
# arrived inside the deadline: rox gives source.radio 15 seconds, and the
# metadata API can take ten on a bad read.
RADIO_RELEASES = 4
RADIO_DEADLINE = 9

# Kept short and plain, so each finds plenty under the Archive's subjects.
GENRES = [
    "ambient", "electronic", "techno", "house", "downtempo", "hip-hop",
    "jazz", "rock", "post-rock", "folk", "experimental", "chiptune",
    "drum and bass", "dub", "idm", "classical",
]

# (id, label, the Archive's sort). The first is the default.
ORDERS = [
    ("popular", "Most downloaded", "downloads desc"),
    ("newest", "Newest", "publicdate desc"),
    ("title", "Title", "titleSorter asc"),
]

SEARCH_VIEWS = [("all", "All"), ("releases", "Releases"), ("labels", "Netlabels")]

# Best first. rox decodes MP3 and Ogg from a plugin, not FLAC, so a lossless
# original plays through one of its derivatives.
PLAYABLE = [
    "320Kbps MP3", "VBR MP3", "256Kbps MP3", "MP3", "192Kbps MP3",
    "128Kbps MP3", "Ogg Vorbis", "64Kbps MP3",
]
AUDIO = set(PLAYABLE) | {"Flac", "24bit Flac", "WAVE", "AIFF", "Apple Lossless Audio"}

# A cover rides one stdout line as base64, and a line is at most 1 MiB.
COVER_CAP = 700 * 1024

READ_CAP = 512 * 1024
STRING_CAP = 1000

ITEM_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,99}$")
REDIRECTS = {301, 302, 303, 307, 308}


class Failure(Exception):
    """An error answer: a JSON-RPC code and a message rox shows or logs."""

    def __init__(self, code, message):
        super().__init__(message)
        self.code = code


def bad_params(message):
    return Failure(-32602, message)


def failed(message):
    return Failure(-32000, message)


out_lock = threading.Lock()
features = set()
place = {"data_dir": None}

# Fan-out inside one request (the home's shelves, a radio batch) runs here,
# apart from the request pool, so a request waiting on it never starves it.
fan = concurrent.futures.ThreadPoolExecutor(max_workers=8)


def send(frame):
    # Several threads answer at once, so each whole line goes out under a lock.
    line = (json.dumps(frame, separators=(",", ":"), ensure_ascii=False) + "\n").encode("utf-8")
    with out_lock:
        sys.stdout.buffer.write(line)
        sys.stdout.buffer.flush()


def log(text):
    sys.stderr.write(text[:4000] + "\n")
    sys.stderr.flush()


def clip(text):
    text = str(text or "").strip()
    return text[:STRING_CAP]


def first(value):
    """The Archive sends one value or a list of them, for the same field."""
    if isinstance(value, list):
        return value[0] if value else ""
    return value or ""


# ── The Archive's APIs ──


def get_json(url, params=None):
    if params:
        url = f"{url}?{urllib.parse.urlencode(params, doseq=True)}"

    request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    try:
        with urllib.request.urlopen(request, timeout=HTTP_TIMEOUT) as response:
            return json.load(response)
    except (OSError, ValueError) as e:
        raise failed(f"the Internet Archive didn't answer: {e}") from e


def get_bytes(url, cap):
    request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    with urllib.request.urlopen(request, timeout=HTTP_TIMEOUT) as response:
        return response.read(cap + 1)


def search(query, sort, page=1, rows=PAGE):
    """One page of search results and the total they're from."""
    answer = get_json(SEARCH_URL, {
        "q": query,
        "fl[]": ["identifier", "title", "creator", "downloads", "year", "date"],
        "sort[]": sort,
        "rows": rows,
        "page": page,
        "output": "json",
    })
    response = answer.get("response") or {}
    return response.get("docs") or [], int(response.get("numFound") or 0)


# Browsing a release, playing it and drawing its cover all read the same
# metadata. A failed read raises, and lru_cache keeps no exceptions.
@functools.lru_cache(maxsize=128)
def metadata(item):
    answer = get_json(METADATA_URL + item)
    if not answer or "metadata" not in answer:
        raise failed(f"no item {item!r} on the Internet Archive")
    return answer


def need_item(item):
    if not ITEM_RE.match(item):
        raise bad_params(f"{item!r} isn't an Internet Archive identifier")
    return item


def free_licence(md):
    licence = str(md.get("licenseurl") or "")
    return "creativecommons.org" in licence or "publicdomain" in licence


# ── Tracks ──


def seconds_of(length):
    """`04:29`, `1:02:03` or `264.71`."""
    text = str(length or "").strip()
    try:
        if ":" in text:
            total = 0.0
            for part in text.split(":"):
                total = total * 60 + float(part)
            return total
        return float(text) if text else 0.0
    except ValueError:
        return 0.0


def number_of(text):
    """`01` or `1/10`."""
    match = re.match(r"\s*(\d+)", str(text or ""))
    return min(int(match.group(1)), 65535) if match else 0


def year_of(md):
    match = re.match(r"\s*(\d{4})", str(first(md.get("year")) or first(md.get("date"))))
    return int(match.group(1)) if match else 0


def genre_of(md):
    subjects = md.get("subject") or []
    if isinstance(subjects, str):
        subjects = re.split(r"[;,]", subjects)

    for subject in subjects:
        if subject.strip().lower() in GENRES:
            return subject.strip().title()
    return ""


def songs(item):
    """The release's tracks as (track, file) pairs, in order. A song the
    Archive holds in several formats plays from the best one rox decodes."""
    answer = metadata(item)
    md = answer["metadata"]
    if not free_licence(md):
        return []

    files = [f for f in answer.get("files") or [] if f.get("format") in AUDIO]
    originals = {f["name"]: f for f in files if f.get("source") == "original"}

    groups = {}
    for f in files:
        root = f["name"] if f.get("source") == "original" else f.get("original", f["name"])
        groups.setdefault(root, []).append(f)

    album = clip(first(md.get("title")))
    album_artist = clip(first(md.get("creator")))
    year = year_of(md)
    genre = genre_of(md)

    found = []
    for root, versions in groups.items():
        playable = [f for f in versions if f.get("format") in PLAYABLE]
        if not playable:
            continue

        best = min(playable, key=lambda f: PLAYABLE.index(f["format"]))
        about = originals.get(root, best)
        secs = seconds_of(about.get("length") or best.get("length"))
        size = int(best.get("size") or 0)

        name = about.get("title") or re.sub(r"\.[^.]+$", "", root.rsplit("/", 1)[-1]).replace("_", " ")
        found.append(({
            "key": f"{item}/{best['name']}",
            "title": clip(name),
            "artist": clip(about.get("creator") or album_artist),
            "album_artist": album_artist,
            "album": album,
            "genre": genre,
            "year": year,
            "disc_no": 1,
            "track_no": number_of(about.get("track")),
            "duration_ms": int(secs * 1000),
            "codec": "Vorbis" if best["format"] == "Ogg Vorbis" else "MP3",
            "bitrate_kbps": min(int(size * 8 / secs / 1000), 65535) if secs > 0 else 0,
            "live": False,
        }, best))

    found.sort(key=lambda pair: (pair[0]["track_no"] or 65535, pair[1]["name"]))
    return found


def tracks_of(item):
    return [track for track, _ in songs(item)]


def split_key(key):
    item, _, name = key.partition("/")
    if not name or ".." in name.split("/"):
        raise bad_params(f"{key!r} isn't a track key")
    return need_item(item), name


# ── Entries, shaped by what this rox reads ──


def node(nid, title, subtitle="", collection=False, kind="", art="", values=None, home=False):
    body = {"id": nid, "title": clip(title), "subtitle": clip(subtitle), "collection": collection}
    if kind and "node-kind" in features:
        body["kind"] = kind
    if art and "node-art" in features:
        body["art"] = art
    if values and "fields" in features:
        body["values"] = values
    if home and "home" in features:
        body["home"] = True
    return {"node": body}


def section(title, entries, tiles=False):
    """The entries under a heading, where this rox draws headings, as a shelf
    of covers where it draws those."""
    if not entries or "sections" not in features:
        return entries

    heading = {"title": title}
    if tiles and "tiles" in features:
        heading["layout"] = "tiles"
    return [{"section": heading}] + entries


def release(doc):
    item = doc["identifier"]
    creator = clip(first(doc.get("creator")))
    year = str(first(doc.get("year")) or str(first(doc.get("date")))[:4])
    subtitle = ", ".join(part for part in (creator, year) if part)

    values = {"downloads": int(doc.get("downloads") or 0)}
    if re.fullmatch(r"\d{4}", year):
        values["year"] = year

    return node(f"item:{item}", first(doc.get("title")) or item, subtitle, True, "album", f"item:{item}", values)


def label(doc):
    item = doc["identifier"]
    return node(f"label:{item}", first(doc.get("title")) or item, "Netlabel", False, "folder", f"label:{item}")


def genre(name):
    return node(f"genre:{name}", name.title(), "", False, "folder")


FIELDS = [
    {"id": "downloads", "label": "Downloads", "kind": "count"},
    {"id": "year", "label": "Year", "kind": "date"},
]


def with_fields(page):
    if "fields" in features:
        page["fields"] = FIELDS
    return page


def offer(page, views, chosen, first_page):
    """The views go on a place's first page only, and only for a rox that reads them."""
    if first_page and "views" in features:
        page["views"] = [{"id": vid, "label": text} for vid, text, *_ in views]
        page["view"] = chosen
    return page


def pick_order(params):
    wanted = params.get("view")
    return next((order for order in ORDERS if order[0] == wanted), ORDERS[0])


def page_cursor(params):
    cursor = params.get("cursor")
    if cursor is None:
        return 1
    if not str(cursor).isdigit() or int(cursor) < 1:
        raise bad_params(f"bad cursor {cursor!r}")
    return int(cursor)


def next_cursor(page, rows, count, total):
    return str(page + 1) if count == rows and page * rows < total else None


def quoted(text):
    return '"' + text.replace("\\", " ").replace('"', " ") + '"'


# ── Methods ──


def hello(params):
    listed = params.get("features")
    features.clear()
    if isinstance(listed, list):
        features.update(f for f in listed if isinstance(f, str))

    place["data_dir"] = params.get("data_dir")
    log(f"hello from rox, api {params.get('api')}, features {sorted(features)}")
    return {"name": "Internet Archive", "version": VERSION, "api": API}


def roots():
    entries = [
        node("home", "Home", "The most downloaded and newest releases", kind="folder", home=True),
        node("labels", "Netlabels", "The labels behind the releases", kind="folder"),
        node("genres", "Genres", "", kind="folder"),
    ]
    page = {"entries": section("Browse", entries), "cursor": None}

    if "notice" in features:
        notice = {
            "text": "Creative Commons and public domain releases from the Internet Archive's "
                    "netlabels collection. Each release's licence is on its page.",
            "kind": "info",
        }
        if "notice-link" in features:
            notice["link"] = {"url": DETAILS_URL + "netlabels", "label": "Netlabels"}
        page["notice"] = notice

    return page


def home():
    """Three searches and the first track of the most downloaded releases, at once."""
    popular = fan.submit(search, RELEASES, "downloads desc", 1, SHELF)
    newest = fan.submit(search, RELEASES, "publicdate desc", 1, SHELF)
    labels = fan.submit(search, LABELS, "downloads desc", 1, SHELF)

    popular_docs = popular.result()[0]
    openers = [fan.submit(tracks_of, doc["identifier"]) for doc in popular_docs[:STARTERS]]

    starters = []
    for opener in openers:
        try:
            tracks = opener.result()
        except Failure as e:
            log(f"a starter failed: {e}")
            continue
        if tracks:
            starters.append({"track": tracks[0]})

    entries = (
        section("Most downloaded", [release(d) for d in popular_docs], tiles=True)
        + section("Tracks to start with", starters)
        + section("Just added", [release(d) for d in newest.result()[0]], tiles=True)
        + section("Netlabels", [label(d) for d in labels.result()[0]], tiles=True)
        + section("Genres", [genre(name) for name in GENRES[:8]])
    )
    return with_fields({"entries": entries, "cursor": None})


def releases_page(query, params, tiles):
    """A place's releases, paged, in the order its view asks for."""
    order = pick_order(params)
    page = page_cursor(params)
    docs, total = search(query, order[2], page)

    entries = [release(d) for d in docs]
    if page == 1 and tiles:
        entries = section("Releases", entries, tiles=True)

    listed = with_fields({"entries": entries, "cursor": next_cursor(page, PAGE, len(docs), total)})
    return offer(listed, ORDERS, order[0], page == 1)


def browse(params):
    nid = params.get("node")

    if nid is None:
        return roots()

    if nid == "home":
        return home()

    if nid == "genres":
        return {"entries": [genre(name) for name in GENRES], "cursor": None}

    if nid == "labels":
        page = page_cursor(params)
        docs, total = search(LABELS, "downloads desc", page)
        entries = [label(d) for d in docs]
        if page == 1:
            entries = section("Netlabels", entries, tiles=True)
        return {"entries": entries, "cursor": next_cursor(page, PAGE, len(docs), total)}

    kind, _, rest = str(nid).partition(":")

    if kind == "item":
        return {"entries": [{"track": t} for t in tracks_of(need_item(rest))], "cursor": None}

    # A label's releases read as a wall of covers, a genre's as rows with
    # their columns: one place shows off each.
    if kind == "label":
        return releases_page(f"collection:{need_item(rest)} AND {RELEASES}", params, tiles=True)

    if kind == "genre" and rest in GENRES:
        return releases_page(f"subject:{quoted(rest)} AND {RELEASES}", params, tiles=False)

    raise bad_params(f"no node {nid!r}")


def search_method(params):
    terms = re.sub(r'[+\-&|!(){}\[\]^"~*?:\\/]', " ", str(params.get("query") or "")).split()
    if not terms:
        return {"entries": [], "cursor": None}

    text = " ".join(terms)
    wanted = params.get("view")
    view = wanted if wanted in {vid for vid, _ in SEARCH_VIEWS} else "all"
    page = page_cursor(params)

    if view == "all":
        releases = fan.submit(search, f"({text}) AND {RELEASES}", "downloads desc", 1, SHELF)
        labels = fan.submit(search, f"({text}) AND {LABELS}", "downloads desc", 1, 6)
        entries = (
            section("Releases", [release(d) for d in releases.result()[0]], tiles=True)
            + section("Netlabels", [label(d) for d in labels.result()[0]], tiles=True)
        )
        return offer({"entries": entries, "cursor": None}, SEARCH_VIEWS, view, True)

    query = f"({text}) AND {RELEASES if view == 'releases' else LABELS}"
    docs, total = search(query, "downloads desc", page)
    shape = release if view == "releases" else label
    listed = with_fields({"entries": [shape(d) for d in docs], "cursor": next_cursor(page, PAGE, len(docs), total)})
    return offer(listed, SEARCH_VIEWS, view, page == 1)


def sync(params):
    collection = str(params.get("collection") or "")
    kind, _, item = collection.partition(":")
    if kind != "item":
        raise bad_params(f"{collection!r} isn't a collection")

    tracks = tracks_of(need_item(item))
    token = hashlib.sha256("\n".join(t["key"] for t in tracks).encode()).hexdigest()[:16]

    if params.get("cursor") is None and params.get("token") == token:
        return {"unchanged": True, "tracks": [], "cursor": None, "token": None}

    return {"unchanged": False, "tracks": tracks, "cursor": None, "token": token}


# ── Streams ──


class Conn:
    """A kept-alive connection that follows redirects, host to host."""

    def __init__(self):
        self.conn = None
        self.origin = None

    def close(self):
        if self.conn is not None:
            self.conn.close()
        self.conn = None
        self.origin = None

    def fetch(self, url, start, end):
        """Bytes start..=end, and the URL they came from after redirects."""
        for _ in range(4):
            parts = urllib.parse.urlsplit(url)
            origin = (parts.scheme, parts.hostname, parts.port)
            if self.conn is None or origin != self.origin:
                self.close()
                self.conn = http.client.HTTPSConnection(parts.hostname, parts.port, timeout=HTTP_TIMEOUT)
                self.origin = origin

            path = (parts.path or "/") + (f"?{parts.query}" if parts.query else "")
            try:
                self.conn.request("GET", path, headers={"User-Agent": USER_AGENT, "Range": f"bytes={start}-{end}"})
                response = self.conn.getresponse()

                if response.status in REDIRECTS and response.getheader("Location"):
                    response.read()
                    url = urllib.parse.urljoin(url, response.getheader("Location"))
                    continue

                body = response.read()
            except (OSError, http.client.HTTPException) as e:
                self.close()
                raise failed(f"{type(e).__name__}: {e}") from e

            if response.will_close:
                self.close()

            if response.status == 206:
                return body, url
            if response.status == 416:
                return b"", url
            raise failed(f"HTTP {response.status} for a range request")

        raise failed("too many redirects")


class Stream:
    def __init__(self, url, length):
        # The storage node the download redirects to, once the first read
        # found it: later reads skip the redirect.
        self.url = url
        self.origin_url = url
        self.length = length
        self.conn = Conn()
        self.lock = threading.Lock()


streams = {}
streams_lock = threading.Lock()
stream_ids = iter(range(1, 1 << 62))


def open_stream(params):
    item, name = split_key(str(params.get("key") or ""))

    # Only a file the release lists is ever fetched, whatever the key says.
    best = next((f for _, f in songs(item) if f["name"] == name), None)
    if best is None:
        raise failed(f"{item} has no playable {name!r}")

    length = int(best.get("size") or 0) or None
    url = DOWNLOAD_URL + item + "/" + urllib.parse.quote(name)

    with streams_lock:
        sid = f"s{next(stream_ids)}"
        streams[sid] = Stream(url, length)

    return {
        "stream": sid,
        "hint": "ogg" if best["format"] == "Ogg Vorbis" else "mp3",
        "length": length,
        "seekable": length is not None,
        "live": False,
    }


def read(params):
    with streams_lock:
        stream = streams.get(params.get("stream"))
    if stream is None:
        raise failed(f"no open stream {params.get('stream')!r}")

    offset = int(params.get("offset") or 0)
    want = min(int(params.get("len") or 0), READ_CAP)

    with stream.lock:
        if want <= 0 or (stream.length is not None and offset >= stream.length):
            return {"data": ""}

        end = offset + want - 1
        if stream.length is not None:
            end = min(end, stream.length - 1)

        try:
            data, stream.url = stream.conn.fetch(stream.url, offset, end)
        except Failure as e:
            # A dropped keep-alive or a storage node gone away: start over
            # from the download URL once before failing the read.
            log(f"read failed, retrying from the start: {e}")
            stream.conn.close()
            data, stream.url = stream.conn.fetch(stream.origin_url, offset, end)

    return {"data": base64.b64encode(data).decode("ascii")}


def close(params):
    with streams_lock:
        stream = streams.pop(params.get("stream"), None)
    if stream is not None:
        with stream.lock:
            stream.conn.close()
    return None


# ── Covers ──


def mime_of(data):
    if data.startswith(b"\xff\xd8"):
        return "image/jpeg"
    if data.startswith(b"\x89PNG"):
        return "image/png"
    if data[:3] == b"GIF":
        return "image/gif"
    return None


def cover_file(item):
    """The release's own cover when it ships one small enough, else None."""
    files = metadata(item).get("files") or []
    images = [
        f for f in files
        if f.get("source") == "original"
        and f.get("format") in ("JPEG", "PNG", "GIF")
        and 0 < int(f.get("size") or 0) <= COVER_CAP
    ]
    if not images:
        return None

    named = [f for f in images if re.search(r"cover|front|folder|art", f["name"], re.I)]
    best = max(named or images, key=lambda f: int(f.get("size") or 0))
    return DOWNLOAD_URL + item + "/" + urllib.parse.quote(best["name"])


@functools.lru_cache(maxsize=256)
def image(url):
    data = get_bytes(url, COVER_CAP)
    return data if len(data) <= COVER_CAP else None


def cover(params):
    key = str(params.get("key") or "")
    kind, _, rest = key.partition(":")

    if kind == "label":
        urls = [THUMB_URL + need_item(rest)]
    else:
        item = need_item(rest) if kind == "item" else split_key(key)[0]
        # The Archive's own tile is 180 px, so a cover the release ships wins.
        urls = [url for url in (cover_file(item), THUMB_URL + item) if url]

    for url in urls:
        try:
            data = image(url)
        except OSError as e:
            log(f"cover {url}: {e}")
            continue

        if data and mime_of(data):
            return {"mime": mime_of(data), "data": base64.b64encode(data).decode("ascii")}

    return None


# ── Radio and links ──


@functools.lru_cache(maxsize=16)
def related(seed):
    """The releases a station plays through, best known first. Kept per seed,
    so every batch of one station reads the same list."""
    kind, _, rest = seed.partition(":")

    if kind == "label":
        return [d["identifier"] for d in search(f"collection:{need_item(rest)} AND {RELEASES}", "downloads desc", 1, 100)[0]]

    if kind == "genre" and rest in GENRES:
        return [d["identifier"] for d in search(f"subject:{quoted(rest)} AND {RELEASES}", "downloads desc", 1, 100)[0]]

    item = need_item(rest) if kind == "item" else split_key(seed)[0]
    md = metadata(item)["metadata"]

    # The same artist, then the same label, then the same genre.
    queries = []
    creator = first(md.get("creator"))
    if creator:
        queries.append(f"creator:{quoted(creator)} AND {RELEASES}")
    labels = [c for c in md.get("collection") or [] if c != "netlabels" and ITEM_RE.match(c)]
    if labels:
        queries.append(f"collection:{labels[0]} AND {RELEASES}")
    kind_of = genre_of(md)
    if kind_of:
        queries.append(f"subject:{quoted(kind_of.lower())} AND {RELEASES}")

    found = []
    for pending in [fan.submit(search, query, "downloads desc", 1, 40) for query in queries]:
        for doc in pending.result()[0]:
            if doc["identifier"] != item and doc["identifier"] not in found:
                found.append(doc["identifier"])
    return found


def radio(params):
    seed = str(params.get("seed") or "")
    count = max(1, min(int(params.get("count") or 20), 500))
    start = page_cursor(params) - 1
    releases = related(seed)

    # A few tracks from each of the next releases, read at once. A release
    # that misses the deadline is skipped, not waited on.
    batch = releases[start:start + RADIO_RELEASES]
    each = max(1, -(-count // RADIO_RELEASES))
    reads = [fan.submit(tracks_of, item) for item in batch]
    concurrent.futures.wait(reads, timeout=RADIO_DEADLINE)

    tracks = []
    for item, pending in zip(batch, reads):
        if not pending.done():
            log(f"radio: {item} took too long, skipped")
            continue
        try:
            tracks.extend(pending.result()[:each])
        except Failure as e:
            log(f"radio: {e}")

    ahead = start + len(batch)
    return {"tracks": tracks[:count], "cursor": str(ahead + 1) if ahead < len(releases) else None}


def link(params):
    item_key = str(params.get("item") or "")
    kind, _, rest = item_key.partition(":")

    if kind in ("item", "label") and ITEM_RE.match(rest):
        return {"url": DETAILS_URL + rest}

    if kind in ("home", "genres", "labels", "genre"):
        return None

    try:
        item, name = split_key(item_key)
    except Failure:
        return None
    return {"url": f"{DETAILS_URL}{item}/{urllib.parse.quote(name)}"}


# ── Download Original ──


downloads = {}
downloads_lock = threading.Lock()
download_ids = iter(range(1, 1 << 62))

# Bytes per read while downloading, so a Stop lands within one.
CHUNK = 256 * 1024


def originals(item, names=None):
    """The original upload behind each of the release's songs, as (item,
    name, size). `names` keeps only the songs whose playable file is named."""
    files = {f["name"]: f for f in metadata(item).get("files") or []}

    found = []
    for _, best in songs(item):
        if names is not None and best["name"] not in names:
            continue

        root = best["name"] if best.get("source") == "original" else best.get("original", best["name"])
        original = files.get(root, best)
        found.append((item, original["name"], int(original.get("size") or 0)))

    return found


def download_folder():
    # Never the plugin's own folder: a write there changes its hash and
    # switches it off. hello hands over one that's ours.
    if not place["data_dir"]:
        raise failed("rox didn't say where this plugin may write")

    folder = os.path.abspath(os.path.join(place["data_dir"], "downloads"))
    os.makedirs(folder, exist_ok=True)
    return folder


def action(params):
    if params.get("action") != "download":
        raise bad_params(f"no action {params.get('action')!r}")

    # Tracks are "<item>/<file>" and releases are "item:<item>"; a label or a
    # genre is too big to download whole.
    names_by_item = {}
    for key in params.get("items") or []:
        kind, _, rest = str(key).partition(":")
        if kind == "item" and ITEM_RE.match(rest):
            names_by_item[rest] = None
        elif ":" not in str(key):
            item, name = split_key(str(key))
            if names_by_item.get(item, set()) is not None:
                names_by_item.setdefault(item, set()).add(name)
        else:
            raise bad_params("only a song or a release downloads")

    files = [f for item, names in names_by_item.items() for f in originals(item, names)]
    if not files:
        raise failed("nothing here to download")

    folder = download_folder()
    reveal = os.path.join(folder, files[0][0]) if len(names_by_item) == 1 else folder

    job = {"done": 0, "total": sum(size for _, _, size in files), "text": "", "stop": False, "end": None}
    with downloads_lock:
        job_id = f"d{next(download_ids)}"
        downloads[job_id] = job

    threading.Thread(target=download, args=(job, files, folder, reveal), daemon=True).start()
    return {"job": job_id}


def download(job, files, folder, reveal):
    try:
        for item, name, _ in files:
            job["text"] = name.rsplit("/", 1)[-1]
            target = os.path.join(folder, item, *name.split("/"))
            os.makedirs(os.path.dirname(target), exist_ok=True)

            if not fetch_to(job, DOWNLOAD_URL + item + "/" + urllib.parse.quote(name), target):
                job["end"] = {"error": "stopped"}
                return
    except OSError as e:
        job["end"] = {"error": f"the download failed: {e}"}
        return

    noun = "file" if len(files) == 1 else "files"
    job["end"] = {"finished": True, "message": f"Downloaded {len(files)} {noun}", "reveal": reveal}


def fetch_to(job, url, target):
    """Writes beside the target and renames at the end, so a stopped or
    failed download never leaves a file that looks whole."""
    part = target + ".part"
    request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})

    with urllib.request.urlopen(request, timeout=HTTP_TIMEOUT) as response, open(part, "wb") as out:
        while chunk := response.read(CHUNK):
            if job["stop"]:
                out.close()
                os.remove(part)
                return False

            out.write(chunk)
            job["done"] += len(chunk)

    os.replace(part, target)
    return True


def job_state(params):
    with downloads_lock:
        job = downloads.get(params.get("job"))
    if job is None:
        raise failed(f"no job {params.get('job')!r}")

    answer = {"done": job["done"], "total": job["total"], "text": job["text"]}
    if job["end"]:
        answer.update(job["end"])
        with downloads_lock:
            downloads.pop(params.get("job"), None)

    return answer


def cancel(params):
    with downloads_lock:
        job = downloads.get(params.get("job"))
    if job is not None:
        job["stop"] = True
    return None


METHODS = {
    "hello": hello,
    "source.browse": browse,
    "source.search": search_method,
    "source.sync": sync,
    "source.open": open_stream,
    "source.read": read,
    "source.close": close,
    "source.cover": cover,
    "source.radio": radio,
    "source.link": link,
    "source.action": action,
    "source.job": job_state,
    "source.cancel": cancel,
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
    pool = concurrent.futures.ThreadPoolExecutor(max_workers=8)

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
    fan.shutdown(wait=False, cancel_futures=True)


if __name__ == "__main__":
    main()
