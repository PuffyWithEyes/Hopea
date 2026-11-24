# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
"""Private Android adapter. Same provider code and protocol as the desktop.

Invoked only in the disposable :import service. No CLI executable, token file,
browser database or FFmpeg subprocess is used on Android.
"""
import contextlib
import hashlib
import json
import logging
import os
from pathlib import Path
import tempfile
import time
from urllib.parse import urlsplit

LIMIT = 256 * 1024
MAX_AUDIO_BYTES = 512 * 1024 * 1024


def download(track, request, directory):
    from java import jclass
    from yt_dlp import aes
    from yt_dlp.downloader import fragment
    crypto = jclass("io.hopea.hopea.ImportService")
    aes.aes_cbc_decrypt_bytes = lambda data, key, iv: bytes(crypto.decryptAesCbc(bytes(data), bytes(key), bytes(iv)))
    # fragment imports the function by value while yt_dlp initializes.
    fragment.aes_cbc_decrypt_bytes = aes.aes_cbc_decrypt_bytes
    from yt_dlp import YoutubeDL
    from service_import import ImportFailure, QuietLogger, cookie_jar
    key = hashlib.sha256((request["service"] + ":" + track["key"]).encode()).hexdigest()
    final = directory / (key + ".flac")
    if final.is_file() and 0 < final.stat().st_size <= MAX_AUDIO_BYTES:
        return final
    url = track["download"]() if track.get("download") else track.get("url")
    if not url or urlsplit(url).scheme != "https":
        return None
    source = directory / (key + ".source")
    deadline = time.monotonic() + 600

    def progress(status):
        if status.get("downloaded_bytes", 0) > MAX_AUDIO_BYTES or time.monotonic() > deadline:
            raise ImportFailure("Audio download exceeded its size or time limit")

    # yt-dlp's native downloader supports direct audio and HLS. Do not launch
    # arbitrary executables from writable storage; local conversion is in Rust.
    with tempfile.TemporaryDirectory(prefix=".partial-", dir=directory) as staging:
        options = dict(quiet=True, no_warnings=True, logger=QuietLogger(),
            noplaylist=True, cachedir=False, socket_timeout=30, retries=2,
            fragment_retries=2, continuedl=False, hls_prefer_native=True,
            format="bestaudio/best", max_filesize=MAX_AUDIO_BYTES,
            buffersize=65536, noresizebuffer=True, progress_hooks=[progress],
            outtmpl=str(Path(staging) / "audio"), postprocessors=[])
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
            # Fail closed instead of falling through to an external downloader.
            if info.get("protocol") not in ("https", "m3u8_native"):
                return None
            downloader.process_ie_result(info, download=True)
        result = Path(staging) / "audio"
        if not result.is_file() or not 0 < result.stat().st_size <= MAX_AUDIO_BYTES:
            return None
        result.replace(source)
    tags = {field: track.get(field) for field in ("title", "artist", "album", "album_artist", "track_number")}
    source.with_suffix(".tags.json").write_text(json.dumps(tags), encoding="utf-8")
    return source


def run(fd, mode):
    # Independent dup descriptors: finishing stdin must not close stdout.
    reader = os.fdopen(os.dup(fd), "rb")
    writer = os.fdopen(fd, "wb")
    logging.disable(logging.CRITICAL)

    def emit(event):
        raw = json.dumps(event, ensure_ascii=False).encode("utf-8") + b"\n"
        if len(raw) > LIMIT:
            raise ValueError("Importer message too large")
        writer.write(raw)
        writer.flush()

    try:
        raw = reader.readline(LIMIT + 1)
        if len(raw) > LIMIT:
            raise ValueError("Import request too large")
        request = json.loads(raw)
        # Discard library prints too: a provider can print signed URLs or tokens.
        with open(os.devnull, "w") as quiet, contextlib.redirect_stdout(quiet), contextlib.redirect_stderr(quiet):
            if mode == 1:
                import service_auth
                if request.get("service") == "spotify":
                    token = service_auth.spotify(request.get("client_id"), emit)
                elif request.get("service") == "yandex":
                    token = service_auth.yandex(emit)
                else:
                    raise ValueError("Unsupported authorization")
                emit(dict(event="credentials", secret=token, account=""))
            elif mode == 0:
                import service_import
                if request.get("browser") or request.get("service") not in service_import.PROVIDERS:
                    raise ValueError("Unsupported importer")
                request["_download"] = download
                service_import.browse(request, emit, iter(lambda: reader.readline(4097), b""))
            else:
                raise ValueError("Unsupported mode")
    except Exception as error:
        # Only adapter-defined errors are sanitized for display. Never serialize
        # a requests/SSL/JSON exception, which can include request bodies.
        import service_auth, service_import
        message = str(error) if isinstance(error, (service_auth.AuthError, service_import.ImportFailure)) else "The service could not be read. Check sign-in data, permissions and connection."
        emit(dict(event="error", message=message))
    finally:
        reader.close()
        writer.close()
