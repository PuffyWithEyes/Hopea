# SPDX-FileCopyrightText: 2026  Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later

"""Account import worker. One JSON request on stdin, JSON events on stdout.

Credentials live only in this process. Provider libraries are loaded lazily;
their logs go to stderr, never into the event stream. The player validates
every downloaded file through the node before adding it to the library.
"""

import contextlib
import hashlib
import http.cookiejar
import json
import logging
import os
from pathlib import Path
import re
import sys
import time
from types import SimpleNamespace
from urllib.parse import urlparse, urljoin

MAX_TRACKS = 20000
MAX_AUDIO_BYTES = 512 * 1024 * 1024
os.environ["YTDLP_NO_PLUGINS"] = "1"


class ImportFailure(Exception):
    pass


class TemporaryServiceFailure(ImportFailure):
    pass


class PartialCatalogFailure(ImportFailure):
    def __init__(self, message, details=()):
        super().__init__(message)
        self.details = details


def clean(value):
    return " ".join(str(value or "").split())[:200]


def song(key, title, artists, url=None, download=None, album=None, album_artist=None,
         track_number=None, cover=None, collection=None, duration_ms=None):
    if isinstance(artists, list):
        artists = ", ".join(
            str(a.get("name", "")) if isinstance(a, dict) else str(a)
            for a in artists
        )
    return dict(key=str(key), title=clean(title), artist=clean(artists) or None, duration_ms=duration_ms,
                url=url, download=download, album=clean(album) or None,
                album_artist=clean(album_artist) or None, track_number=track_number,
                cover=cover, collections=[collection] if collection else [])


def collection(kind, key, title, cover=None):
    return {"key": f"{kind}:{key}", "kind": kind, "title": clean(title) or str(key), "cover": cover}


def picture(images):
    return next((i.get("url") for i in sorted(images or [], key=lambda i: i.get("width", 0), reverse=True)
                 if i.get("url")), None)


def spotify(request):
    import spotipy
    client = spotipy.Spotify(auth=request["secret"], requests_timeout=45,
                             retries=2, status_retries=2, backoff_factor=2)

    def pages(page):
        seen = set()
        while page:
            yield from page.get("items", [])
            next_page = page.get("next")
            if not next_page:
                return
            if next_page in seen or not next_page.startswith("https://api.spotify.com/v1/"):
                raise ImportFailure("Spotify returned an invalid pagination link")
            seen.add(next_page)
            page = client.next(page)

    def tracks(items, group, album=None):
        for item in items:
            track = item.get("track", item.get("item", item))
            if track and track.get("type", "track") == "track":
                record = track.get("album") or album or {}
                yield song(track.get("id") or track.get("uri"), track.get("name"),
                           track.get("artists", []), album=record.get("name"),
                           album_artist=", ".join(a.get("name", "") for a in record.get("artists", [])),
                           track_number=track.get("track_number"), cover=picture(record.get("images")),
                           collection=group, duration_ms=track.get("duration_ms"))

    yield from tracks(pages(client.current_user_saved_tracks(limit=50)), collection("library", "saved", "Saved songs"))
    for playlist in pages(client.current_user_playlists(limit=50)):
        yield from tracks(pages(client.playlist_items(playlist["id"], limit=50)),
                          collection("playlist", playlist["id"], playlist.get("name"), picture(playlist.get("images"))))
    for saved in pages(client.current_user_saved_albums(limit=50)):
        album = saved["album"]
        yield from tracks(pages(client.album_tracks(album["id"], limit=50)),
                          collection("album", album["id"], album.get("name"), picture(album.get("images"))), album)


def yandex_read(request, stage, operation, *args):
    """Retry only timed-out reads; keep diagnostics independent of provider data."""
    from yandex_music.exceptions import TimedOutError, UnauthorizedError

    def progress(message):
        if request.get("_reading_catalog"):
            request["_progress"](message)

    progress(f"Reading Yandex Music {stage}…")
    for attempt in range(3):
        try:
            return operation(*args)
        except UnauthorizedError:
            raise ImportFailure(f"Yandex Music refused access while reading {stage}. Sign in again. If this persists, the token may not permit this operation; Hopea will not request broader permissions automatically.") from None
        except TimedOutError:
            if attempt == 2:
                raise TemporaryServiceFailure(f"Yandex Music did not respond while reading {stage} after 3 attempts. Check access to api.music.yandex.net and your connection or VPN. A timeout does not establish that sign-in or permissions are invalid.") from None
        progress(f"Waiting for Yandex Music {stage}. Retrying ({attempt + 2}/3)…")
        time.sleep(2 * (attempt + 1))


def yandex(request):
    from yandex_music import Client
    client = Client(request["secret"])
    client.request.set_timeout(45)
    read = lambda stage, operation, *args: yandex_read(request, stage, operation, *args)
    read("account status", client.init)

    def tracks(shorts, group, album=None):
        # Fetch metadata in batches; a liked-tracks response holds IDs only.
        ids = [str(t.id) for t in shorts]
        positions = {key: i + 1 for i, key in enumerate(ids)} if album else {}
        for start in range(0, len(ids), 100):
            for track in read("track details", client.tracks, ids[start:start + 100]) or []:
                def download_url(track=track):
                    choices = [d for d in read("audio availability", track.get_download_info) or []
                               if d.codec == "mp3"]
                    if not choices:
                        return None
                    return read("audio link", max(choices, key=lambda d: d.bitrate_in_kbps).get_direct_link)
                record = album or next(iter(getattr(track, "albums", []) or []), None)
                cover = getattr(track, "cover_uri", None) or getattr(record, "cover_uri", None)
                if cover:
                    cover = "https://" + cover.removeprefix("https://").replace("%%", "600x600")
                position = getattr(track, "track_position", None) or getattr(record, "track_position", None)
                yield song(track.id, track.title, [a.name for a in track.artists], download=download_url,
                           album=getattr(record, "title", None),
                           album_artist=", ".join(a.name for a in getattr(record, "artists", []) or []),
                           track_number=getattr(position, "index", None) or positions.get(str(track.id)), cover=cover, collection=group,
                           duration_ms=getattr(track, "duration_ms", None))

    likes = read("liked songs", client.users_likes_tracks)
    if likes:
        yield from tracks(likes.tracks, collection("library", "likes", "Liked songs"))
    playlists = list(read("playlists", client.users_playlists_list) or [])
    playlists.extend(like.playlist for like in read("saved playlists", client.users_likes_playlists) or []
                     if like.playlist)
    seen = set()
    for summary in playlists:
        key = (summary.owner.uid, summary.kind)
        if key in seen:
            continue
        seen.add(key)
        playlist = read("playlist details", client.users_playlists, summary.kind, summary.owner.uid)
        if playlist:
            yield from tracks(read("playlist songs", playlist.fetch_tracks), collection("playlist", f"{key[0]}:{key[1]}",
                                                                 getattr(playlist, "title", None)))
    for like in read("saved albums", client.users_likes_albums) or []:
        album = read("album songs", client.albums_with_tracks, like.album.id) if like.album else None
        if album:
            yield from tracks([track for volume in album.volumes for track in volume],
                              collection("album", like.album.id, getattr(album, "title", None)), album)


def cookie_jar(header, domain):
    from http.cookies import SimpleCookie
    parsed = SimpleCookie()
    parsed.load(header)
    jar = http.cookiejar.CookieJar()
    for name, value in parsed.items():
        jar.set_cookie(http.cookiejar.Cookie(
            0, name, value.value, None, False, domain, True, domain.startswith("."),
            "/", True, True, None, True, None, None, {}, False))
    return jar


def vk_playlists(session, owner):
    """Use the same paginated JSON method as VK's AudioPage.loadPlaylists.

    The older mobile HTML scraper silently returns zero for current pages.
    Template scripts returned alongside these records are never evaluated.
    """
    offset, seen = 0, set()
    while True:
        response = session.post("https://vk.ru/al_audio.php?act=owner_playlists", data={
            "al": 1, "owner_id": int(owner), "group_id": 0,
            "offset": offset, "is_attach": 0,
        }, headers={"X-Requested-With": "XMLHttpRequest"}, allow_redirects=False)
        try:
            payload = response.json().get("payload")
        except (ValueError, AttributeError):
            raise ImportFailure("VK returned a verification page instead of albums. Open your music in the default browser and retry.") from None
        if not isinstance(payload, list) or len(payload) != 2 or str(payload[0]) != "0":
            raise ImportFailure("VK did not allow reading the albums. The song list was kept; sign in again and retry.")
        data = payload[1]
        if (not isinstance(data, list) or len(data) < 2 or not isinstance(data[0], list)
                or not isinstance(data[1], int) or isinstance(data[1], bool) or data[1] < 0):
            raise ImportFailure("VK changed the album response format. The song list was kept.")
        items, total = data[:2]
        if total > MAX_TRACKS:
            raise ImportFailure("The library exceeds 20,000 albums; narrow the source collection.")
        for item in items:
            if (not isinstance(item, dict) or not re.fullmatch(r"-?[1-9][0-9]*", str(item.get("owner_id", "")))
                    or not re.fullmatch(r"[1-9][0-9]*", str(item.get("id", "")))):
                raise ImportFailure("VK returned an invalid album; the partial collection was kept.")
            key = (int(item["owner_id"]), int(item["id"]))
            if key in seen:
                raise ImportFailure("VK repeated an album page; the partial collection was kept.")
            seen.add(key)
            yield item
        offset += len(items)
        if offset >= total:
            return
        if not items:
            raise ImportFailure("VK reported more albums but returned an empty page; retry later.")


def vk(request):
    import requests
    from bs4 import BeautifulSoup
    from vk_api.audio import VkAudio, scrap_ids, scrap_tracks
    from vk_api.vk_api import DEFAULT_USERAGENT
    owner = request.get("account", "").strip()
    if not re.fullmatch(r"[1-9][0-9]*", owner):
        raise ImportFailure("Enter your numeric VK user ID")
    session = requests.Session()
    credential = request["secret"].strip()
    viewer = ""
    if credential.startswith("{"):
        credential = json.loads(credential)
        viewer = str(credential.get("viewer_id", ""))
        for cookie in credential.get("cookies", []):
            domain = cookie.get("domain", "")
            if domain.lstrip(".") in ("vk.com", "vk.ru", "m.vk.com", "m.vk.ru"):
                session.cookies.set(cookie["name"], cookie["value"], domain=domain,
                                    path=cookie.get("path", "/"), secure=True)
        session.headers["User-Agent"] = credential.get("user_agent") or DEFAULT_USERAGENT
    else:
        # Compatibility with manually pasted cookies. Account identity is
        # separate from the profile being imported.
        for domain in (".vk.com", ".vk.ru"):
            session.cookies.update(cookie_jar(credential, domain))
        session.headers["User-Agent"] = DEFAULT_USERAGENT
    if not viewer:
        viewer = next((c.value for c in session.cookies if c.name == "remixmid"), "")
    if not any(c.name in ("remixsid", "remixsid6") and c.value for c in session.cookies):
        raise ImportFailure("The VK session is missing. Sign in with your default browser again.")
    if not re.fullmatch(r"[1-9][0-9]*", viewer):
        viewer, user_agent = vk_browser_identity(list(session.cookies))
        if user_agent:
            session.headers["User-Agent"] = user_agent
    original_request = session.request
    reloaded_covers = {}

    def timed_request(method, url, **kwargs):
        kwargs.setdefault("timeout", (15, 45))
        action = (kwargs.get("data") or {}).get("act") if isinstance(kwargs.get("data"), dict) else None
        stage = ("VK song list" if action == "load_section" else
                 "VK track details" if action == "reload_audio" else "VK albums and playlists")
        def progress(message):
            if request.get("_reading_catalog"):
                request["_progress"](message)
        progress(f"Reading {stage}…")
        for attempt in range(3):
            try:
                response = original_request(method, url, **kwargs)
            except (requests.Timeout, requests.ConnectionError):
                if attempt == 2:
                    raise TemporaryServiceFailure(f"No response while reading {stage} after 3 attempts. Check your connection or VPN and try again later. This does not necessarily mean your sign-in is invalid.") from None
            else:
                if response.status_code not in (502, 503, 504):
                    break
                response.close()
                if attempt == 2:
                    raise TemporaryServiceFailure(f"{stage} is temporarily unavailable. Try again later.")
            progress(f"Waiting for {stage}. Retrying ({attempt + 2}/3)…")
            time.sleep(2 * (attempt + 1))
        if response.status_code in (301, 302, 303, 307, 308, 401, 403):
            raise ImportFailure("VK redirected or refused the music request. Sign in again and open the source profile’s audio page in your default browser.")
        if response.status_code == 429:
            raise ImportFailure("VK is limiting requests. Wait a few minutes and retry.")
        if response.status_code != 200:
            raise ImportFailure(f"VK returned HTTP {response.status_code}; the collection was not read.")
        if isinstance(kwargs.get("data"), dict) and kwargs["data"].get("act") in ("load_section", "reload_audio"):
            if response.text.lstrip().startswith("<"):
                raise ImportFailure("VK returned a sign-in or verification page instead of songs. Complete the check in your default browser and retry.")
            if kwargs["data"].get("act") == "reload_audio":
                # Read artwork before vk_api tries to decode a playable URL.
                # An unavailable or undecodable audio URL must not hide art.
                data = response.json().get("data")
                rows = data[0] if isinstance(data, list) and data else []
                if isinstance(rows, list):
                    for row in rows:
                        if isinstance(row, list) and len(row) > 14 and isinstance(row[14], str):
                            reloaded_covers[f"{row[1]}_{row[0]}"] = next(
                                (url.strip() for url in reversed(row[14].split(",")) if url.strip()), None)
        return response

    session.request = timed_request

    def method(name):
        if name != "users.get":
            raise ImportFailure("The VK adapter requested an unexpected method")
        return [{"id": int(viewer)}]

    audio = VkAudio(SimpleNamespace(http=session, method=method), convert_m3u8_links=False)
    last_reload = [0.0]

    def tracks(owner_id, playlist_id=-1, access_hash=None, group=None):
        offset = 0
        pages = set()
        while True:
            response = session.post("https://m.vk.ru/audio", data={
                "act": "load_section", "owner_id": owner_id, "playlist_id": playlist_id,
                "offset": offset, "type": "playlist", "access_hash": access_hash,
                "is_loading_all": 1,
            }, headers={"X-Requested-With": "XMLHttpRequest"}, allow_redirects=False).json()
            data = response.get("data")
            if isinstance(data, list) and data and data[0] is False:
                raise ImportFailure("VK refused access to this collection for the signed-in account. The album may be private, removed or restricted; open it in VK to check.")
            if not isinstance(data, list) or not data or not isinstance(data[0], dict):
                raise ImportFailure("VK did not grant access to this music collection, or changed its response format.")
            page = data[0]
            rows = page.get("list")
            if not isinstance(rows, list):
                raise ImportFailure("VK returned an unrecognized song list; import was not completed.")
            signature = tuple((str(r[1]), str(r[0])) for r in rows if isinstance(r, list) and len(r) >= 5)
            if rows and (not signature or signature in pages):
                raise ImportFailure("VK returned an invalid or repeated song page; the partial collection was kept.")
            pages.add(signature)
            for row in rows:
                if not isinstance(row, list) or len(row) < 5:
                    raise ImportFailure("VK changed the song format; the partial collection was kept.")
                # Metadata does not require a playable URL. The library's
                # get_iter silently skips rows with missing download hashes.
                resolved = []
                def details(row=row, cache=resolved):
                    if cache:
                        return cache[0]
                    try:
                        ids = scrap_ids([row])
                    except (IndexError, AttributeError):
                        return {}
                    if not ids:
                        return {}
                    # scrap_tracks limits requests within one call; selections
                    # issue separate calls, so keep the same delay across them.
                    time.sleep(max(0, 1.5 - (time.monotonic() - last_reload[0])))
                    last_reload[0] = time.monotonic()
                    result = next(scrap_tracks(ids, int(viewer), session, convert_m3u8_links=False), None)
                    cache.append(result or {})
                    return cache[0]
                def metadata(details=details, key=f"{row[1]}_{row[0]}"):
                    try:
                        covers = details().get("track_covers") or []
                    except Exception:
                        if key not in reloaded_covers:
                            raise
                        covers = []
                    return reloaded_covers.get(key) or next((url for url in reversed(covers) if url), None)
                item = song(f"{row[1]}_{row[0]}", BeautifulSoup(str(row[3]), "html.parser").get_text(),
                           BeautifulSoup(str(row[4]), "html.parser").get_text(),
                           cover=(row[14].split(",")[-1] if len(row) > 14 and isinstance(row[14], str) and row[14] else None),
                    collection=group, duration_ms=int(row[5]) * 1000 if len(row) > 5 and str(row[5]).isdigit() else None)
                item["download"] = lambda details=details: details().get("url")
                item["refresh_cover"] = metadata
                yield item
            if not page.get("hasMore"):
                break
            if not rows:
                raise ImportFailure("VK reported more songs but returned an empty page; retry later.")
            offset += len(rows)

    yield from tracks(int(owner), group=collection("library", owner, "Audio library"))
    unavailable = []
    for playlist in vk_playlists(session, owner):
        key = (playlist["owner_id"], playlist["id"])
        group = collection("album" if playlist.get("is_official") else "collection",
                           f"{key[0]}:{key[1]}",
                           BeautifulSoup(str(playlist.get("title") or ""), "html.parser").get_text(),
                           playlist.get("thumb") or None)
        try:
            # Do not offer half an album as a complete, selectable collection.
            # Keep the per-collection buffer bounded independently of the
            # merged catalogue's unique-song limit.
            album_tracks = []
            for item in tracks(playlist["owner_id"], playlist["id"], playlist.get("access_hash"), group):
                if len(album_tracks) >= MAX_TRACKS:
                    raise ImportFailure("This album exceeds 20,000 songs.")
                album_tracks.append(item)
        except ImportFailure as error:
            # One removed/private playlist must not hide the rest of the shelf.
            unavailable.append((group["title"], str(error)))
        else:
            yield from album_tracks
    if unavailable:
        raise PartialCatalogFailure(
            f"{len(unavailable)} VK albums could not be fully read. These albums were excluded; other songs and complete albums are ready for selection.",
            [f"• {title}: {clean(reason)}" for title, reason in unavailable])



def youtube(request):
    from ytmusicapi import YTMusic
    headers = json.loads(request["secret"])
    if not isinstance(headers, dict) or not any(k.lower() == "cookie" for k in headers):
        raise ImportFailure("Paste the YouTube Music browser session headers as JSON")
    client = YTMusic(auth=headers)

    def tracks(items, group, album=None):
        for index, track in enumerate(items, 1):
            ident = track.get("videoId")
            if ident and re.fullmatch(r"[A-Za-z0-9_-]{11}", ident):
                record = album or track.get("album") or {}
                if isinstance(record, str):
                    record = {"name": record}
                yield song(ident, track.get("title"), track.get("artists", []),
                           url=f"https://music.youtube.com/watch?v={ident}",
                           album=record.get("title") or record.get("name"),
                           album_artist=", ".join(a.get("name", "") for a in record.get("artists", [])),
                           track_number=(index if album else track.get("trackNumber")),
                           cover=picture(track.get("thumbnails") or record.get("thumbnails")), collection=group,
                           duration_ms=int(track["duration_seconds"]) * 1000 if track.get("duration_seconds") else None)
        if len(items) >= MAX_TRACKS:
            raise ImportFailure("A YouTube Music section reached 20,000 songs; the partial import was kept")

    yield from tracks(client.get_library_songs(limit=MAX_TRACKS), collection("library", "saved", "Library songs"))
    yield from tracks(client.get_liked_songs(limit=MAX_TRACKS).get("tracks", []), collection("library", "likes", "Liked songs"))
    playlists = client.get_library_playlists(limit=None)
    albums = client.get_library_albums(limit=MAX_TRACKS)
    seen = set()
    for item in playlists:
        ident = item.get("playlistId")
        if ident and ident not in seen:
            seen.add(ident)
            yield from tracks(client.get_playlist(ident, limit=MAX_TRACKS).get("tracks", []),
                              collection("playlist", ident, item.get("title")))
    for item in albums:
        ident = item.get("browseId")
        if ident:
            album = client.get_album(ident)
            yield from tracks(album.get("tracks", []), collection("album", ident, album.get("title"), picture(album.get("thumbnails"))), album)
        elif item.get("playlistId") and item["playlistId"] not in seen:
            ident = item["playlistId"]
            seen.add(ident)
            yield from tracks(client.get_playlist(ident, limit=MAX_TRACKS).get("tracks", []),
                              collection("album", ident, item.get("title")), item)


PROVIDERS = {"vk": vk, "yandex": yandex, "spotify": spotify, "youtube": youtube}


class QuietLogger:
    def debug(self, message):
        pass

    info = warning = error = debug


def vk_page_identity(page):
    # Desktop VK merges its bootstrap with Object.assign(window.vk, {...});
    # mobile VK can still assign the object directly. Never execute page JS
    # or mistake a profile's cur.oid / an analytics user_id for the viewer.
    assignment = r'\bvk\s*=\s*(?:Object\.assign\(\s*(?:window\.)?vk\s*(?:\|\|\s*\{\s*\}\s*)?,\s*)?'
    for match in re.finditer(assignment + r'(?=\{)', page):
        value = page[match.end():]
        try:
            obj, _ = json.JSONDecoder().raw_decode(value)
            identity = str(obj.get("id", "")) if isinstance(obj, dict) else ""
        except ValueError:
            # Legacy JS objects use unquoted property names. Restrict the
            # fallback to the first level, before any nested object ends.
            field = re.search(r'''^\{[^{}]*?(?:["']id["']|\bid\b)\s*:\s*["']?([0-9]+)\b''', value)
            identity = field[1] if field else ""
        if re.fullmatch(r"[1-9][0-9]*", identity):
            return identity
    return None


def vk_browser_identity(cookies):
    """Some VK sessions omit remixmid; read the signed-in bootstrap identity."""
    import requests
    from vk_api.vk_api import DEFAULT_USERAGENT
    session = requests.Session()
    session.headers["User-Agent"] = DEFAULT_USERAGENT
    for cookie in cookies:
        session.cookies.set_cookie(cookie)
    location = "https://vk.ru/audio"
    try:
        for _ in range(5):
            parsed = urlparse(location)
            if (parsed.scheme != "https" or parsed.hostname not in ("vk.ru", "vk.com", "m.vk.ru", "m.vk.com")
                    or parsed.username or parsed.password or parsed.port not in (None, 443)):
                break
            with session.get(location, timeout=(15, 30), allow_redirects=False, stream=True) as response:
                if response.status_code in (301, 302, 303, 307, 308):
                    location = urljoin(location, response.headers.get("Location", ""))
                    continue
                if response.status_code != 200:
                    break
                data = bytearray()
                for chunk in response.iter_content(65536):
                    data.extend(chunk)
                    if len(data) > 4 * 1024 * 1024:
                        raise ImportFailure("VK’s sign-in page is too large to read safely")
                identity = vk_page_identity(data.decode("utf-8", errors="replace"))
                if identity:
                    return identity, DEFAULT_USERAGENT
                break
    except requests.RequestException:
        raise ImportFailure("Unable to verify the VK browser session. Check the connection and try Continue again.") from None
    finally:
        session.close()
    raise ImportFailure("VK did not confirm a signed-in account. Sign in at vk.ru in the browser’s default profile, open your music and try Continue again.")


def browser_session(request):
    from yt_dlp import YoutubeDL
    from yt_dlp.cookies import extract_cookies_from_browser, YDLLogger
    service = request.get("service")
    if service not in ("vk", "youtube"):
        raise ImportFailure("This service requires an access token")
    try:
        with YoutubeDL({"quiet": True, "no_warnings": True, "logger": QuietLogger(), "cachedir": False}) as ydl:
            profile = request.get("browser_profile")
            if request["browser"] == "firefox" and profile:
                # yt-dlp copies only cookies.sqlite. A just-completed Zen sign-in
                # can still be in its WAL; consolidate a private copy first.
                import shutil
                import sqlite3
                import tempfile
                with tempfile.TemporaryDirectory(prefix="hopea-browser-") as temp:
                    source = Path(profile) / "cookies.sqlite"
                    snapshot = Path(temp) / "snapshot.sqlite"
                    shutil.copyfile(source, snapshot)
                    if Path(str(source) + "-wal").is_file():
                        shutil.copyfile(str(source) + "-wal", str(snapshot) + "-wal")
                    with contextlib.closing(sqlite3.connect(snapshot)) as read, contextlib.closing(sqlite3.connect(Path(temp) / "cookies.sqlite")) as merged:
                        read.backup(merged)
                    jar = extract_cookies_from_browser("firefox", profile=temp, logger=YDLLogger(ydl))
            else:
                jar = extract_cookies_from_browser(request["browser"], logger=YDLLogger(ydl))
    except Exception:
        raise ImportFailure("Could not read the default browser’s saved session. Close the browser if its database is locked, then retry.") from None
    domains = ("vk.ru", "vk.com") if service == "vk" else ("youtube.com",)
    cookies = [c for c in jar if not c.is_expired() and any(
        c.domain.lstrip(".") == d or c.domain.lstrip(".").endswith("." + d) for d in domains)]
    if service == "vk":
        if not any(c.name in ("remixsid", "remixsid6") and c.value for c in cookies):
            raise ImportFailure("No signed-in VK session was found. Sign in in a normal window of your default browser.")
        account = next((c.value for c in cookies if c.name == "remixmid"), "")
        user_agent = None
        if not re.fullmatch(r"[1-9][0-9]*", account):
            account, user_agent = vk_browser_identity(cookies)
        secret = json.dumps({"viewer_id": account, "user_agent": user_agent, "cookies": [
            {"name": c.name, "value": c.value, "domain": c.domain, "path": c.path}
            for c in cookies]})
    else:
        if not any(c.name in ("SAPISID", "__Secure-3PAPISID") and c.value for c in cookies):
            raise ImportFailure("No signed-in YouTube Music session was found. Sign in in a normal window of your default browser.")
        account = ""
        secret = json.dumps({"cookie": "; ".join(f"{c.name}={c.value}" for c in cookies),
                             "authorization": "SAPISIDHASH", "x-goog-authuser": "0",
                             "x-origin": "https://music.youtube.com", "content-type": "application/json"})
    return {"event": "credentials", "secret": secret, "account": account}


def download(track, request, directory):
    """Only final FLAC files become checkpoints; failed partials are retried."""
    from yt_dlp import YoutubeDL
    key = hashlib.sha256((request["service"] + ":" + track["key"]).encode()).hexdigest()
    target = directory / f"{key}.flac"
    if target.is_file() and target.stat().st_size:
        if target.stat().st_size > MAX_AUDIO_BYTES:
            raise ImportFailure("Downloaded audio exceeds the file size limit")
        return target
    url = track["download"]() if track.get("download") else track.get("url")
    if not url:
        return None
    if urlparse(url).scheme not in ("http", "https"):
        return None
    # Each download has its own staging folder. A failed conversion is
    # never mistaken for a completed file on the next run.
    import tempfile
    with tempfile.TemporaryDirectory(prefix=".partial-", dir=directory) as staging:
        deadline = time.monotonic() + 600

        def check_progress(progress):
            # max_filesize alone checks a reported length, not all streaming
            # paths. Count bytes received even when total_bytes is false/absent.
            if progress.get("downloaded_bytes", 0) > MAX_AUDIO_BYTES:
                raise ImportFailure("Downloaded audio exceeds the file size limit")
            if time.monotonic() > deadline:
                raise ImportFailure("Audio download exceeded its time limit")

        options = {
            "quiet": True, "no_warnings": True, "logger": QuietLogger(),
            "noplaylist": True, "cachedir": False, "socket_timeout": 30,
            "retries": 2, "fragment_retries": 2, "continuedl": False,
            "format": "bestaudio/best", "max_filesize": MAX_AUDIO_BYTES,
            "buffersize": 65536, "noresizebuffer": True,
            "progress_hooks": [check_progress],
            "outtmpl": str(Path(staging) / "audio.%(ext)s"),
            "postprocessor_args": {
                "extractaudio+ffmpeg_i": ["-max_alloc", "67108864", "-protocol_whitelist", "file,pipe"],
                "extractaudio+ffmpeg_o": ["-fs", str(MAX_AUDIO_BYTES + 1), "-t", "10801"],
                "metadata+ffmpeg_i": ["-max_alloc", "67108864", "-protocol_whitelist", "file,pipe"],
                "metadata+ffmpeg_o": ["-fs", str(MAX_AUDIO_BYTES + 1)],
            },
            "postprocessors": [
                {"key": "FFmpegExtractAudio", "preferredcodec": "flac"},
                {"key": "FFmpegMetadata", "add_metadata": True},
            ],
        }
        with YoutubeDL(options) as downloader:
            if request["service"] == "youtube":
                headers = json.loads(request["secret"])
                cookie = next((v for k, v in headers.items() if k.lower() == "cookie"), "")
                for entry in cookie_jar(cookie, ".youtube.com"):
                    downloader.cookiejar.set_cookie(entry)
            info = downloader.extract_info(url, download=False)
            if not info or info.get("_type") == "playlist":
                return None
            if (info.get("duration") or 0) > 10800:
                raise ImportFailure("Audio exceeds the three-hour duration limit")
            info.update(title=track["title"], track=track["title"], artist=track["artist"])
            info.update({k: track[k] for k in ("album", "album_artist", "track_number") if track.get(k)})
            downloader.process_ie_result(info, download=True)
        final = Path(staging) / "audio.flac"
        if not final.is_file() or final.stat().st_size == 0:
            return None
        if final.stat().st_size > MAX_AUDIO_BYTES:
            raise ImportFailure("Converted audio exceeds the file size limit")
        final.replace(target)
    return target


def run(request, emit):
    service = request.get("service")
    if service not in PROVIDERS:
        raise ImportFailure("Unknown music service")
    if request.get("download") and service != "spotify":
        import yt_dlp  # Fail early, instead of marking every file unavailable.
        import shutil
        if not shutil.which("ffmpeg"):
            raise ImportFailure("FFmpeg is missing. See IMPORTING.md.")
    directory = Path(request["directory"])
    directory.mkdir(parents=True, exist_ok=True)
    seen = set()
    total = downloaded = unavailable = 0
    emit({"event": "progress", "message": "Reading your library…"})
    for track in PROVIDERS[service](request):
        if not track["title"] or track["key"] in seen:
            continue
        if total >= MAX_TRACKS:
            raise ImportFailure("Import reached 20,000 songs; the partial collection was kept")
        seen.add(track["key"])
        total += 1
        # Deliver the song before downloading it so cancellation still
        # preserves its name for P2P search.
        emit({"event": "track", "title": track["title"], "artist": track["artist"]})
        emit({"event": "progress", "message": f'{total}: {track["artist"] or ""} — {track["title"]}'})
        if not request.get("download"):
            continue
        try:
            path = download(track, request, directory) if service != "spotify" else None
        except Exception:
            # Provider exceptions may contain tokens, cookies or signed URLs.
            # Report the affected song and a fixed explanation, never str(exc).
            path = None
        if path:
            downloaded += 1
            emit({"event": "file", "path": str(path), "title": track["title"],
                  "artist": track["artist"]})
        else:
            unavailable += 1
    if service == "vk" and total == 0:
        raise ImportFailure("VK returned no songs for profile " + request.get("account", "") +
                            ". Open that profile’s audio page in your default browser with the same account. If songs are visible there, the VK parser could not read them; this is not a successful import.")
    emit({"event": "done", "total": total, "downloaded": downloaded,
          "unavailable": unavailable})


def browse(request, emit, commands):
    """Keep provider download callbacks in memory until Rust requests a selected track."""
    if request.get("download") and request["service"] != "spotify":
        import shutil
        import yt_dlp
        if not shutil.which("ffmpeg"):
            raise ImportFailure("FFmpeg is missing. See IMPORTING.md.")
    request = dict(request, _reading_catalog=True, _progress=lambda message: emit({"event": "progress", "message": message}))
    catalog = {}
    membership_positions, next_positions = {}, {}
    def readable_tracks():
        try:
            yield from PROVIDERS[request["service"]](request)
        except (TemporaryServiceFailure, PartialCatalogFailure) as error:
            if not catalog:
                raise
            summary = "Only part of the library was read. You can import the songs already found or cancel selection and retry later. " + str(error)
            details = getattr(error, "details", ())
            # Separate bounded events preserve every album warning without
            # exceeding the IPC line limit for large libraries.
            emit({"event": "warning", "message": summary + ("\n" + details[0] if details else "")})
            for detail in details[1:]:
                emit({"event": "warning", "message": detail})
    for track in readable_tracks():
        if not track["title"]:
            continue
        key = track["key"]
        memberships = []
        for group in track["collections"]:
            group = dict(group)
            membership = (group["key"], key)
            if membership not in membership_positions:
                position = next_positions.get(group["key"], 0)
                membership_positions[membership] = position
                next_positions[group["key"]] = position + 1
            group["position"] = membership_positions[membership]
            memberships.append(group)
        track["collections"] = memberships
        if key in catalog:
            old = catalog[key]
            for group in track["collections"]:
                if group not in old["collections"]:
                    old["collections"].append(group)
            for field in ("album", "album_artist", "track_number", "cover"):
                old[field] = old.get(field) or track.get(field)
        else:
            if len(catalog) >= MAX_TRACKS:
                raise ImportFailure("The library exceeds 20,000 songs; narrow the source collection.")
            catalog[key] = track
        if len(catalog) % 100 == 0:
            emit({"event": "progress", "message": f"Reading library: {len(catalog)} songs"})
    request["_reading_catalog"] = False
    if not catalog:
        raise ImportFailure("The service returned no accessible songs. Check your session and source profile.")
    for track in catalog.values():
        emit({"event": "catalog_track", "track": {k: v for k, v in track.items() if k not in ("url", "download", "refresh_cover")}})
    emit({"event": "ready"})
    directory = Path(request["directory"])
    directory.mkdir(parents=True, exist_ok=True)
    for raw in commands:
        if len(raw) > 4096:
            raise ImportFailure("Invalid import command")
        command = json.loads(raw)
        if command.get("finish"):
            return
        key = command.get("key")
        if key not in catalog:
            raise ImportFailure("Unknown selected song")
        if command.get("metadata"):
            track = catalog[key]
            failed = False
            try:
                if request.get("covers", True) and track.get("refresh_cover"):
                    track["cover"] = track["refresh_cover"]() or track.get("cover")
            except Exception:
                # A metadata failure must not prevent importing accessible audio.
                failed = True
            emit({"event": "metadata", "key": key, "cover": track.get("cover") if request.get("covers", True) else None, "failed": failed})
            continue
        try:
            path = download(catalog[key], request, directory) if request["service"] != "spotify" else None
        except Exception:
            path = None
        emit({"event": "downloaded", "key": key, "path": str(path) if path else None})


def main():
    if os.name == "posix":
        import signal
        os.setsid()

        def stop(_signal, _frame):
            signal.signal(signal.SIGTERM, signal.SIG_IGN)
            os.killpg(os.getpgrp(), signal.SIGTERM)
            raise SystemExit(130)

        signal.signal(signal.SIGTERM, stop)
    output = sys.stdout

    def emit(event):
        output.write(json.dumps(event, ensure_ascii=False) + "\n")
        output.flush()

    logging.disable(logging.CRITICAL)
    try:
        raw = sys.stdin.buffer.readline(256 * 1024 + 1)
        if len(raw) > 256 * 1024:
            raise ImportFailure("The sign-in data is too large")
        request = json.loads(raw)
        with contextlib.redirect_stdout(sys.stderr):
            if request.get("browser"):
                emit(browser_session(request))
            elif request.get("browse"):
                browse(request, emit, iter(lambda: sys.stdin.buffer.readline(4097), b""))
            else:
                run(request, emit)
    except ModuleNotFoundError as error:
        emit({"event": "error", "message": f"Missing import dependency: {error.name}. See IMPORTING.md."})
        return 1
    except ImportFailure as error:
        emit({"event": "error", "message": str(error)})
        return 1
    except Exception as error:
        emit({"event": "error", "message": "The service could not be read. Check sign-in data, permissions and connection. "
              + f"({type(error).__name__})"})
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
