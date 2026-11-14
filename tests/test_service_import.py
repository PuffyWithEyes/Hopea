# SPDX-FileCopyrightText: 2026  Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later

"""Offline protocol/adapter tests; no accounts, browser profiles or network."""

import importlib.util
import contextlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace as Obj
import unittest
from unittest.mock import patch

SOURCE = Path(__file__).resolve().parents[1] / "src/library/service_import.py"
spec = importlib.util.spec_from_file_location("service_import", SOURCE)
worker = importlib.util.module_from_spec(spec)
spec.loader.exec_module(worker)


class ReadTimeout(Exception):
    pass


@contextlib.contextmanager
def vk_responses(pages, status=200, text="json", playlists=(), resolved=None, timeout_calls=()):
    """Fake provider boundary; exercise our pagination and credential handling."""
    calls, viewers = [], []
    responses = iter(pages)
    class Session:
        def __init__(self):
            self.cookies = worker.cookie_jar("", ".vk.ru")
            self.cookies.update = lambda jar: [self.cookies.set_cookie(c) for c in jar]
            self.headers = {}
        def request(self, method, url, **kwargs):
            if "act=owner_playlists" in url:
                offset = kwargs["data"]["offset"]
                return Obj(status_code=200, text="json", json=lambda: {"payload": [0, [list(playlists)[offset:offset+50], len(playlists), {}]]}, close=lambda: None)
            calls.append(kwargs)
            if len(calls) in timeout_calls:
                raise ReadTimeout("sensitive-cookie-and-url-must-not-escape")
            return Obj(status_code=status, text=text, json=lambda: next(responses), close=lambda: None)
        def post(self, url, **kwargs):
            return self.request("POST", url, **kwargs)
    def audio(vk, **_):
        viewers.append(vk.method("users.get")[0]["id"])
        return Obj(get_albums_iter=lambda **_: iter(playlists))
    modules = {"requests": Obj(Session=Session, Timeout=ReadTimeout, ConnectionError=ConnectionError),
               "bs4": Obj(BeautifulSoup=lambda text, _: Obj(get_text=lambda: text)),
               "vk_api": Obj(), "vk_api.vk_api": Obj(DEFAULT_USERAGENT="Test"),
               "vk_api.audio": Obj(VkAudio=audio, scrap_ids=lambda rows: rows if resolved else [],
                                   scrap_tracks=resolved or (lambda *_a, **_k: iter([])))}
    with patch.dict(sys.modules, modules):
        yield calls, viewers


class ImportTests(unittest.TestCase):
    def test_all_album_warnings_survive_beyond_first_five(self):
        albums = [{"owner_id": 42, "id": n + 1, "title": f"Unavailable {n}"} for n in range(8)]
        pages = [{"data": [{"list": [[1, 42, "", "Song", "Artist"]], "hasMore": False}]}]
        pages.extend({"data": []} for _ in albums)
        events = []
        with tempfile.TemporaryDirectory() as directory, vk_responses(pages, playlists=albums):
            worker.browse({"service": "vk", "account": "42", "secret": "remixsid=test; remixmid=123", "directory": directory}, events.append, [json.dumps({"finish": True})])
        warnings = [e["message"] for e in events if e["event"] == "warning"]
        self.assertEqual(len(warnings), 8)
        for n in range(8):
            self.assertTrue(any(f"Unavailable {n}:" in message for message in warnings))
        self.assertEqual(events[-1]["event"], "ready")

    def test_vk_structured_albums_all_pages_covers_and_no_script_execution(self):
        items = [{"owner_id": 42, "id": n+1, "title": f"Album {n}", "thumb": "https://images.example/cover.jpg"} for n in range(61)]
        offsets = []
        def post(url, **kwargs):
            self.assertEqual(url, "https://vk.ru/al_audio.php?act=owner_playlists")
            offset = kwargs["data"]["offset"]; offsets.append(offset)
            return Obj(json=lambda: {"payload": [0, [items[offset:offset+50], 61, {"templates_script": "raise Exception('never execute')"}]]})
        result = list(worker.vk_playlists(Obj(post=post), "42"))
        self.assertEqual(offsets, [0, 50])
        self.assertEqual(len(result), 61)
        self.assertEqual(result[-1]["thumb"], "https://images.example/cover.jpg")

    def test_vk_invalid_empty_and_repeated_album_pages_are_not_success(self):
        item = {"owner_id": 42, "id": 1, "title": "One"}
        for response in [{"payload": [3, ["private-session-value"]]},
                         {"payload": [0, [[], 1]]}, {"payload": [0, [[item], 2]]},
                         {"payload": [0, [[{"id": "bad"}], 1]]}]:
            with self.assertRaises(worker.ImportFailure) as caught:
                list(worker.vk_playlists(Obj(post=lambda *a, **k: Obj(json=lambda: response)), 42))
            self.assertNotIn("private-session-value", str(caught.exception))
        self.assertEqual(list(worker.vk_playlists(Obj(post=lambda *a, **k: Obj(json=lambda: {"payload":[0,[[],0]]})), 42)), [])

    def test_vk_desktop_bootstrap_identifies_viewer_without_executing_js(self):
        for page in [
            'window.vk = Object.assign(window.vk, {ads_rotate_interval: 120000, al: 1, id: 123, sex: 0});',
            'vk = Object.assign(window.vk || {}, {"config": {}, "id": 123});',
            'var vk = {"config": {"id": 999}, "id": 123};',
        ]:
            self.assertEqual(worker.vk_page_identity(page), '123')
        for page in [
            'var cur = {oid: 123}; var analytics = {"user_id": 123};',
            'window.vk = Object.assign(window.vk, {id: 0}); var cur = {id: 123};',
            'vk = {"config": {"id": 999}};',
        ]:
            self.assertIsNone(worker.vk_page_identity(page))

    def test_vk_timeout_retries_same_page_without_losing_selection(self):
        page = {"data": [{"list": [[1, 123456789, "", "One", "Artist"]], "hasMore": False}]}
        events = []
        with tempfile.TemporaryDirectory() as directory, vk_responses([page], timeout_calls={1}) as (calls, _), patch.object(worker.time, "sleep"):
            worker.browse({"service": "vk", "account": "123456789", "secret": "remixsid=test; remixmid=123", "directory": directory}, events.append, [json.dumps({"finish": True})])
        self.assertEqual(len(calls), 2)
        self.assertEqual(calls[0]["data"], calls[1]["data"])
        self.assertTrue(any("Retrying (2/3)" in e.get("message", "") for e in events))
        self.assertEqual(len([e for e in events if e["event"] == "catalog_track"]), 1)
        self.assertNotIn("sensitive-cookie", json.dumps(events))

    def test_vk_exhausted_timeout_keeps_partial_catalog_with_warning(self):
        page = {"data": [{"list": [[1, 123456789, "", "One", "Artist"]], "hasMore": True}]}
        events = []
        with tempfile.TemporaryDirectory() as directory, vk_responses([page], timeout_calls={2, 3, 4}) as (calls, _), patch.object(worker.time, "sleep"):
            worker.browse({"service": "vk", "account": "123456789", "secret": "remixsid=test; remixmid=123", "directory": directory}, events.append, [json.dumps({"finish": True})])
        self.assertEqual([call["data"]["offset"] for call in calls], [0, 1, 1, 1])
        self.assertEqual(len([e for e in events if e["event"] == "catalog_track"]), 1)
        self.assertTrue(any(e["event"] == "warning" for e in events))
        self.assertEqual(events[-1]["event"], "ready")
        self.assertNotIn("sensitive-cookie", json.dumps(events))

    def test_vk_timeout_without_results_is_not_success(self):
        events = []
        with tempfile.TemporaryDirectory() as directory, vk_responses([], timeout_calls={1, 2, 3}) as (calls, _), patch.object(worker.time, "sleep"):
            with self.assertRaisesRegex(worker.TemporaryServiceFailure, "after 3 attempts"):
                worker.browse({"service": "vk", "account": "123456789", "secret": "remixsid=test; remixmid=123", "directory": directory}, events.append, [])
        self.assertEqual(len(calls), 3)
        self.assertFalse(any(e["event"] == "ready" for e in events))

    def test_vk_art_survives_unavailable_audio_url(self):
        row = [1, 123456789, "", "Song", "Artist"] + [None] * 10
        full = list(row)
        full[14] = "https://sun9.userapi.com/small.jpg,https://sun9.userapi.com/large.jpg"
        pages = [{"data": [{"list": [row], "hasMore": False}]}, {"data": [[full]]}]
        def undecodable(rows, viewer, session, **_):
            session.post("https://m.vk.ru/audio", data={"act": "reload_audio", "ids": "selected"})
            raise ValueError("Audio URL cannot be decoded")
        with vk_responses(pages, resolved=undecodable):
            track = next(worker.vk({"account": "123456789", "secret": "remixsid=session; remixmid=123"}))
            self.assertEqual(track["refresh_cover"](), "https://sun9.userapi.com/large.jpg")

    def test_covers_disabled_keeps_audio_import_and_skips_cover_lookup(self):
        track = worker.song("1", "Song", "Artist", cover="https://sun9.userapi.com/image.jpg")
        def no_cover():
            self.fail("cover metadata must not be requested")
        track["refresh_cover"] = no_cover
        downloaded = []
        events = []
        def download(track, *_):
            downloaded.append(track["key"])
            return Path("/tmp/audio.flac")
        with tempfile.TemporaryDirectory() as directory, patch.dict(worker.PROVIDERS, {"vk": lambda _: iter([track])}), patch.object(worker, "download", download):
            worker.browse({"service": "vk", "covers": False, "directory": directory}, events.append,
                          [json.dumps({"key": "1", "metadata": True}), json.dumps({"key": "1"}), json.dumps({"finish": True})])
        self.assertEqual(downloaded, ["1"])
        metadata = next(e for e in events if e["event"] == "metadata")
        self.assertIsNone(metadata["cover"])
        self.assertFalse(metadata["failed"])

    def test_vk_unavailable_album_does_not_hide_other_collections(self):
        row = lambda ident: [ident, 42, "", "Song", "Artist"]
        pages = [{"data":[{"list":[row(1)],"hasMore":False}]}, {"data":[]},
                 {"data":[{"list":[row(2)],"hasMore":False}]}]
        events=[]
        with tempfile.TemporaryDirectory() as directory, vk_responses(pages, playlists=[{"owner_id":42,"id":1,"title":"Unavailable"},{"owner_id":42,"id":2,"title":"Available","thumb":"https://images.example/cover.jpg"}]):
            worker.browse({"service":"vk","account":"42","secret":"remixsid=test; remixmid=123","directory":directory},events.append,[json.dumps({"finish":True})])
        tracks=[e['track'] for e in events if e['event']=='catalog_track']
        self.assertEqual(len(tracks),2)
        self.assertEqual(tracks[1]['collections'][0]['title'],'Available')
        self.assertEqual(tracks[1]['collections'][0]['cover'],'https://images.example/cover.jpg')
        self.assertTrue(any(e['event']=='warning' and '1 VK albums' in e['message'] for e in events))
        warning = next(e['message'] for e in events if e['event'] == 'warning')
        self.assertIn('Unavailable:', warning)
        self.assertIn('VK did not grant access', warning)
        self.assertEqual(events[-1]['event'],'ready')

    def test_vk_partial_album_is_not_offered_as_complete_and_later_albums_survive(self):
        row = lambda ident: [ident, 42, "", "Song", "Artist"]
        pages = [{"data":[{"list":[row(1)],"hasMore":False}]},
                 {"data":[{"list":[row(2)],"hasMore":True}]}, {"data":[]},
                 {"data":[{"list":[row(3)],"hasMore":False}]}]
        events=[]
        with tempfile.TemporaryDirectory() as directory, vk_responses(pages, playlists=[
            {"owner_id":42,"id":1,"title":"Incomplete"}, {"owner_id":42,"id":2,"title":"Complete"}]):
            worker.browse({"service":"vk","account":"42","secret":"remixsid=test; remixmid=123","directory":directory},events.append,[json.dumps({"finish":True})])
        tracks=[e['track'] for e in events if e['event']=='catalog_track']
        self.assertEqual([track['key'] for track in tracks], ['42_1', '42_3'])
        self.assertIn('Incomplete:',next(e['message'] for e in events if e['event']=='warning'))
        self.assertEqual(events[-1]['event'],'ready')

    def test_vk_full_collections_and_late_covers_without_downloading_audio(self):
        # Section metadata has no cover. reload_audio supplies it separately.
        row = lambda ident: [ident, 123456789, "", f"Song {ident}", "Artist"]
        pages = [{"data": [{"list": [row(1)], "hasMore": False}]},
                 {"data": [{"list": [row(1), row(2)], "hasMore": True}]},
                 {"data": [{"list": [row(3)], "hasMore": False}]}]
        lookups = []
        def resolved(rows, *_a, **_k):
            ident = rows[0][0]
            lookups.append(ident)
            return iter([{"url": f"https://audio.example/{ident}",
                          "track_covers": ["", f"https://sun9.userapi.com/{ident}.jpg"]}])
        events = []
        with tempfile.TemporaryDirectory() as directory, vk_responses(
                pages, playlists=[{"owner_id": 12, "id": 8, "title": "Full record", "access_hash": "hash"}],
                resolved=resolved) as (calls, _), patch.object(worker, "download", side_effect=AssertionError("metadata must not download audio")):
            worker.browse({"service": "vk", "account": "123456789", "secret": "remixsid=session; remixmid=123", "directory": directory},
                          events.append, [json.dumps({"key": "123456789_1", "metadata": True}),
                                          json.dumps({"key": "123456789_2", "metadata": True}),
                                          json.dumps({"key": "123456789_1", "metadata": True}),
                                          json.dumps({"finish": True})])
        catalog = [e["track"] for e in events if e["event"] == "catalog_track"]
        self.assertEqual(len(catalog), 3)
        self.assertTrue(all(any(c["title"] == "Full record" for c in t["collections"]) for t in catalog))
        self.assertEqual([c["data"]["offset"] for c in calls], [0, 0, 2])
        self.assertEqual(lookups, [1, 2])
        self.assertNotIn("refresh_cover", json.dumps(catalog))
        covers = [e["cover"] for e in events if e["event"] == "metadata"]
        self.assertEqual(covers, ["https://sun9.userapi.com/1.jpg", "https://sun9.userapi.com/2.jpg", "https://sun9.userapi.com/1.jpg"])

    def test_browse_merges_membership_and_downloads_only_selected_keys(self):
        liked = worker.collection("library", "likes", "Likes")
        album = worker.collection("album", "a", "Full Album")
        tracks = [worker.song("1", "One", "Artist", collection=liked),
                  worker.song("1", "One", "Artist", album="Full Album", cover="https://i.scdn.co/image/1", collection=album),
                  worker.song("2", "Two", "Artist", album="Full Album", collection=album)]
        events, downloads = [], []
        def download(track, *_):
            downloads.append(track["key"])
            return Path("/tmp/selected.flac")
        with tempfile.TemporaryDirectory() as directory, patch.dict(worker.PROVIDERS, {"vk": lambda _: iter(tracks)}), patch.object(worker, "download", download):
            worker.browse({"service": "vk", "directory": directory}, events.append,
                          [json.dumps({"key": "2"}), json.dumps({"finish": True})])
        catalog = [e["track"] for e in events if e["event"] == "catalog_track"]
        self.assertEqual(len(catalog), 2)
        self.assertEqual(catalog[0]["collections"], [dict(liked, position=0), dict(album, position=0)])
        self.assertEqual(catalog[0]["album"], "Full Album")
        self.assertNotIn("download", catalog[0])
        self.assertNotIn("url", catalog[0])
        self.assertEqual(downloads, ["2"])
        self.assertEqual(events[-1]["event"], "downloaded")

    def test_browse_preserves_collection_order_independent_of_library_order(self):
        liked = worker.collection("library", "likes", "Likes")
        album = worker.collection("album", "a", "Full Album")
        tracks = [worker.song("2", "Two", "Artist", collection=liked),
                  worker.song("1", "One", "Artist", collection=album),
                  worker.song("2", "Two", "Artist", collection=album)]
        events = []
        with tempfile.TemporaryDirectory() as directory, patch.dict(worker.PROVIDERS, {"vk": lambda _: iter(tracks)}):
            worker.browse({"service": "vk", "directory": directory}, events.append, [json.dumps({"finish": True})])
        catalog = [e["track"] for e in events if e["event"] == "catalog_track"]
        order = {t["key"]: next(c["position"] for c in t["collections"] if c["key"] == album["key"]) for t in catalog}
        self.assertEqual(order, {"1": 0, "2": 1})

    def test_browse_without_selection_never_downloads(self):
        track = worker.song("1", "One", "Artist")
        with tempfile.TemporaryDirectory() as directory, patch.dict(worker.PROVIDERS, {"vk": lambda _: iter([track])}), patch.object(worker, "download", side_effect=AssertionError("no selection")):
            worker.browse({"service": "vk", "directory": directory}, lambda _: None,
                          [json.dumps({"finish": True})])

    def test_youtube_complete_album_browse_id_metadata_and_covers(self):
        client = Obj(get_library_songs=lambda **_: [], get_liked_songs=lambda **_: {},
            get_library_playlists=lambda **_: [], get_library_albums=lambda **_: [{"browseId": "MPRE_album"}],
            get_album=lambda ident: {"title": "Record", "artists": [{"name": "Artist"}],
                "thumbnails": [{"url": "https://i.ytimg.com/cover.jpg", "width": 600}],
                "tracks": [{"videoId": str(i) * 11, "title": f"Song {i}", "artists": [{"name": "Artist"}]} for i in range(3)]})
        with patch.dict(sys.modules, {"ytmusicapi": Obj(YTMusic=lambda **_: client)}):
            songs = list(worker.youtube({"secret": '{"cookie":"test"}'}))
        self.assertEqual(len(songs), 3)
        self.assertEqual([s["track_number"] for s in songs], [1, 2, 3])
        self.assertTrue(all(s["album"] == "Record" and s["cover"] and s["collections"][0]["kind"] == "album" for s in songs))
    def test_vk_source_profile_and_viewer_are_distinct_and_missing_urls_keep_metadata(self):
        pages = [{"data": [{"list": [[1, 123456789, "", "First", "Artist"]], "hasMore": True}]},
                 {"data": [{"list": [[2, 123456789, "", "Second", "Artist"]], "hasMore": False}]}]
        with vk_responses(pages) as (calls, viewers):
            songs = list(worker.vk({"account": "123456789", "secret": "remixsid=session; remixmid=123"}))
            self.assertTrue(all(song["download"]() is None for song in songs))
        self.assertEqual(viewers, [123])
        self.assertEqual([call["data"]["owner_id"] for call in calls], [123456789, 123456789])
        self.assertEqual([call["data"]["offset"] for call in calls], [0, 1])
        self.assertEqual([s["title"] for s in songs], ["First", "Second"])

    def test_manual_vk_cookie_without_remixmid_verifies_signed_in_user(self):
        pages = [{"data": [{"list": [[1, 123456789, "", "First", "Artist"]], "hasMore": False}]}]
        with vk_responses(pages) as (calls, viewers), patch.object(worker, "vk_browser_identity", return_value=("123", "Test browser")) as identity:
            songs = list(worker.vk({"account": "123456789", "secret": "remixsid6=test"}))
            identity.assert_called_once()
        self.assertEqual(viewers, [123])
        self.assertEqual(calls[0]["data"]["owner_id"], 123456789)
        self.assertEqual(len(songs), 1)

    def test_vk_rejects_auth_html_redirect_and_repeated_pages(self):
        request = {"account": "123456789", "secret": "remixsid=session; remixmid=123"}
        for status, text in [(302, ""), (200, "<html>Sign in</html>"), (429, "")]:
            with self.subTest(status=status), vk_responses([], status, text):
                with self.assertRaises(worker.ImportFailure):
                    list(worker.vk(request))
        page = {"data": [{"list": [[1, 123456789, "", "Song", "Artist"]], "hasMore": True}]}
        with vk_responses([page, page]):
            songs = worker.vk(request)
            self.assertEqual(next(songs)["title"], "Song")
            with self.assertRaisesRegex(worker.ImportFailure, "repeated"):
                next(songs)

    def test_vk_empty_is_not_reported_as_success(self):
        events = []
        with tempfile.TemporaryDirectory() as directory, patch.dict(worker.PROVIDERS, {"vk": lambda _: iter([])}):
            with self.assertRaisesRegex(worker.ImportFailure, "123456789"):
                worker.run({"service": "vk", "account": "123456789", "directory": directory}, events.append)
        self.assertNotIn("done", [event["event"] for event in events])

    def test_default_browser_session_filters_other_sites(self):
        jar = list(worker.cookie_jar("remixsid=vk-session; remixmid=123", ".vk.ru"))
        jar += list(worker.cookie_jar("private=other-site", ".example.com"))
        jar += list(worker.cookie_jar("private=lookalike", ".fakevk.ru"))
        class Downloader:
            def __init__(self, _): pass
            def __enter__(self): return self
            def __exit__(self, *_): pass
        modules = {"yt_dlp": Obj(YoutubeDL=Downloader), "yt_dlp.cookies": Obj(
            extract_cookies_from_browser=lambda *_a, **_k: jar, YDLLogger=lambda ydl: ydl)}
        with patch.dict(sys.modules, modules):
            event = worker.browser_session({"service": "vk", "browser": "firefox"})
        secret = json.loads(event["secret"])
        self.assertEqual(event["account"], "123")
        self.assertEqual(secret["viewer_id"], "123")
        self.assertEqual({c["domain"] for c in secret["cookies"]}, {".vk.ru"})
        self.assertNotIn("other-site", event["secret"])
        self.assertNotIn("lookalike", event["secret"])

    @unittest.skipUnless(importlib.util.find_spec("yt_dlp"), "requires import environment")
    def test_real_firefox_reader_with_synthetic_profile(self):
        import sqlite3
        import time
        from yt_dlp import cookies
        with tempfile.TemporaryDirectory() as profile:
            with sqlite3.connect(Path(profile) / "cookies.sqlite") as db:
                db.execute("CREATE TABLE moz_cookies (host TEXT, name TEXT, value TEXT, path TEXT, expiry INTEGER, isSecure INTEGER)")
                db.executemany("INSERT INTO moz_cookies VALUES (?, ?, ?, '/', ?, 1)", [
                    (".vk.ru", "remixsid", "synthetic-session", int(time.time()) + 3600),
                    (".vk.ru", "remixmid", "123", int(time.time()) + 3600),
                    (".example.com", "unrelated", "must-not-return", int(time.time()) + 3600)])
            db.close()
            # Never discover or read the user's browser directories.
            with patch.object(cookies, "_firefox_browser_dirs", side_effect=AssertionError("must use Zen's explicit profile")):
                event = worker.browser_session({"service": "vk", "browser": "firefox", "browser_profile": profile})
        self.assertEqual(event["account"], "123")
        self.assertNotIn("must-not-return", event["secret"])

    @unittest.skipUnless(importlib.util.find_spec("yt_dlp"), "requires import environment")
    def test_zen_reads_fresh_login_while_browser_wal_is_open(self):
        import sqlite3
        import time
        with tempfile.TemporaryDirectory() as profile:
            db = sqlite3.connect(Path(profile) / "cookies.sqlite")
            try:
                db.execute("pragma journal_mode=WAL")
                db.execute("CREATE TABLE moz_cookies (host TEXT, name TEXT, value TEXT, path TEXT, expiry INTEGER, isSecure INTEGER)")
                db.execute("pragma wal_checkpoint(TRUNCATE)")
                db.executemany("INSERT INTO moz_cookies VALUES ('.vk.ru', ?, ?, '/', ?, 1)", [
                    ("remixsid", "fresh-login", int(time.time()) + 3600),
                    ("remixmid", "123", int(time.time()) + 3600)])
                db.commit()
                self.assertTrue(Path(profile, "cookies.sqlite-wal").is_file())
                result = worker.browser_session({"service": "vk", "browser": "firefox", "browser_profile": profile})
                self.assertEqual(result["account"], "123")
                # The reader must leave the browser connection usable.
                self.assertEqual(db.execute("select count(*) from moz_cookies").fetchone()[0], 2)
            finally:
                db.close()

    def test_vk_browser_identity_without_remixmid_and_safe_redirects(self):
        calls = []
        class Response:
            def __init__(self, status, body=b"", location=""):
                self.status_code, self.body = status, body
                self.headers = {"Location": location}
            def __enter__(self): return self
            def __exit__(self, *_): pass
            def iter_content(self, *_): return iter([self.body])
        responses = []
        class Session:
            def __init__(self):
                self.headers = {}
                self.cookies = worker.cookie_jar("", ".vk.ru")
            def get(self, url, **kwargs):
                calls.append(url)
                selftest.assertFalse(kwargs["allow_redirects"])
                return responses.pop(0)
            def close(self): pass
        selftest = self
        modules = {"requests": Obj(Session=Session, RequestException=RuntimeError),
                   "vk_api": Obj(), "vk_api.vk_api": Obj(DEFAULT_USERAGENT="Test browser")}
        with patch.dict(sys.modules, modules):
            responses.extend([Response(302, location="/feed"), Response(200, b'var vk = {"id": 123}; var cur = {"oid": 123456789};')])
            self.assertEqual(worker.vk_browser_identity(list(worker.cookie_jar("remixsid=test", ".vk.ru"))), ("123", "Test browser"))
            self.assertEqual(calls, ["https://vk.ru/audio", "https://vk.ru/feed"])
            calls.clear()
            responses.append(Response(302, location="https://vk.ru.evil.example/login"))
            with self.assertRaises(worker.ImportFailure): worker.vk_browser_identity([])
            self.assertEqual(len(calls), 1)
            responses.append(Response(200, b'var vk = {id: 0, oid: 123456789};'))
            with self.assertRaises(worker.ImportFailure): worker.vk_browser_identity([])

    def test_browser_session_uses_verified_viewer_when_cookie_is_missing(self):
        jar = list(worker.cookie_jar("remixsid=session", ".vk.ru"))
        class Downloader:
            def __init__(self, _): pass
            def __enter__(self): return self
            def __exit__(self, *_): pass
        modules = {"yt_dlp": Obj(YoutubeDL=Downloader), "yt_dlp.cookies": Obj(
            extract_cookies_from_browser=lambda *_a, **_k: jar, YDLLogger=lambda ydl: ydl)}
        with patch.dict(sys.modules, modules), patch.object(worker, "vk_browser_identity", return_value=("123", "Test browser")):
            result = worker.browser_session({"service": "vk", "browser": "firefox"})
        self.assertEqual(result["account"], "123")
        self.assertEqual(json.loads(result["secret"])["user_agent"], "Test browser")

    def test_spotify_all_pages_playlists_albums_and_null_tracks(self):
        track = lambda ident: {"id": ident, "name": ident, "artists": [{"name": "Artist"}]}
        page = lambda items, next=None: {"items": items, "next": next}
        client = Obj(
            current_user_saved_tracks=lambda **_: page([{"track": track("liked")}], "https://api.spotify.com/v1/next"),
            next=lambda _: page([{"track": None}, {"track": track("second-page")}]),
            current_user_playlists=lambda **_: page([{"id": "playlist"}]),
            playlist_items=lambda *_, **__: page([{"item": track("playlist-song")}, {"item": {"type": "episode"}}]),
            current_user_saved_albums=lambda **_: page([{"album": {"id": "album", "name": "Album",
                "artists": [{"name": "Artist"}], "images": [{"url": "https://i.scdn.co/image/cover", "width": 600}]}}]),
            album_tracks=lambda *_, **__: page([track("album-song")]),
        )
        with patch.dict(sys.modules, {"spotipy": Obj(Spotify=lambda **_: client)}):
            songs = list(worker.spotify({"secret": "do-not-log"}))
        self.assertEqual([s["key"] for s in songs], ["liked", "second-page", "playlist-song", "album-song"])
        self.assertTrue(all(s["artist"] == "Artist" for s in songs))
        self.assertEqual(songs[-1]["album"], "Album")
        self.assertEqual(songs[-1]["cover"], "https://i.scdn.co/image/cover")
        self.assertEqual(songs[-1]["collections"][0]["kind"], "album")

    def test_spotify_rejects_cross_origin_pagination_before_sending_token(self):
        client = Obj(current_user_saved_tracks=lambda **_: {"items": [], "next": "https://evil.example/page"})
        with patch.dict(sys.modules, {"spotipy": Obj(Spotify=lambda **_: client)}):
            with self.assertRaises(worker.ImportFailure):
                list(worker.spotify({"secret": "do-not-log"}))

    def test_yandex_batches_likes_and_reads_saved_album_id(self):
        batches = []
        albums = []
        def tracks(ids):
            batches.append(ids)
            return [Obj(id=i, title=i, artists=[Obj(name="Artist")]) for i in ids]
        def album(ident):
            albums.append(ident)
            return Obj(volumes=[[Obj(id="album-track")]], title="Album", artists=[Obj(name="Artist")], cover_uri="avatars.yandex.net/cover/%%")
        client = Obj(
            tracks=tracks,
            users_likes_tracks=lambda: Obj(tracks=[Obj(id=str(i)) for i in range(205)]),
            users_playlists_list=lambda: [Obj(owner=Obj(uid=1), kind=7)],
            users_likes_playlists=lambda: [Obj(playlist=Obj(owner=Obj(uid=1), kind=7))],
            users_playlists=lambda *_: Obj(fetch_tracks=lambda: [Obj(id="playlist-track")]),
            users_likes_albums=lambda: [Obj(id="like-id", album=Obj(id="album-id"))],
            albums_with_tracks=album,
        )
        with patch.dict(sys.modules, {"yandex_music": Obj(Client=lambda _: Obj(init=lambda: client))}):
            songs = list(worker.yandex({"secret": "do-not-log"}))
        self.assertEqual(len(songs), 207)
        self.assertEqual([len(b) for b in batches], [100, 100, 5, 1, 1])
        self.assertEqual(albums, ["album-id"])
        self.assertEqual(songs[-1]["album"], "Album")
        self.assertEqual(songs[-1]["track_number"], 1)
        self.assertEqual(songs[-1]["cover"], "https://avatars.yandex.net/cover/600x600")

    def test_youtube_reads_all_collection_sections(self):
        def track(ident):
            return {"videoId": ident * 11, "title": ident, "artists": [{"name": "Artist"}]}
        calls = []
        def playlist(ident, **kwargs):
            calls.append(ident)
            self.assertEqual(kwargs["limit"], worker.MAX_TRACKS)
            return {"tracks": [track(ident)]}
        client = Obj(
            get_library_songs=lambda **_: [track("a")],
            get_liked_songs=lambda **_: {"tracks": [track("b")]},
            get_library_playlists=lambda **_: [{"playlistId": "c"}],
            get_library_albums=lambda **_: [{"playlistId": "c"}, {"playlistId": "d"}],
            get_playlist=playlist,
        )
        with patch.dict(sys.modules, {"ytmusicapi": Obj(YTMusic=lambda **_: client)}):
            songs = list(worker.youtube({"secret": '{"cookie":"test"}'}))
        self.assertEqual([s["title"] for s in songs], list("abcd"))
        self.assertEqual(calls, ["c", "d"])

    def test_duplicates_are_not_imported_or_downloaded_twice(self):
        events = []
        tracks = [worker.song("1", "First", "Artist"), worker.song("1", "First", "Artist"),
                  worker.song("2", "Second", "Artist")]
        with tempfile.TemporaryDirectory() as directory:
            with patch.dict(worker.PROVIDERS, {"spotify": lambda _: iter(tracks)}):
                worker.run({"service": "spotify", "download": True, "directory": directory}, events.append)
        self.assertEqual([e["title"] for e in events if e["event"] == "track"], ["First", "Second"])
        self.assertEqual(events[-1], {"event": "done", "total": 2, "downloaded": 0, "unavailable": 2})

    def test_partial_collection_survives_provider_failure(self):
        def provider(_):
            yield worker.song("1", "First", "Artist")
            raise RuntimeError("secret-url-must-not-escape")
        events = []
        with tempfile.TemporaryDirectory() as directory, patch.dict(worker.PROVIDERS, {"spotify": provider}):
            with self.assertRaises(RuntimeError):
                worker.run({"service": "spotify", "directory": directory}, events.append)
        self.assertEqual([e["title"] for e in events if e["event"] == "track"], ["First"])
        self.assertNotIn("done", [e["event"] for e in events])

    def test_protocol_errors_do_not_echo_credentials(self):
        result = subprocess.run([sys.executable, "-I", str(SOURCE)],
                                input=json.dumps({"service": "unknown", "secret": "TOKEN-SECRET"}),
                                capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 1)
        event = json.loads(result.stdout)
        self.assertEqual(event["event"], "error")
        self.assertNotIn("TOKEN-SECRET", result.stdout + result.stderr)

    def test_download_uses_checkpoint_without_requesting_expired_url(self):
        track = worker.song("../../id", "Track", "Artist", download=lambda: self.fail("checkpoint must be reused"))
        request = {"service": "vk"}
        with tempfile.TemporaryDirectory() as directory, patch.dict(sys.modules, {"yt_dlp": Obj(YoutubeDL=None)}):
            root = Path(directory)
            digest = worker.hashlib.sha256(b"vk:../../id").hexdigest()
            final = root / f"{digest}.flac"
            final.write_bytes(b"completed-file")
            self.assertEqual(worker.download(track, request, root), final)

    def test_cookie_scope_is_not_a_global_authorization_header(self):
        jar = worker.cookie_jar("remixsid=private; remixmid=123", ".vk.ru")
        self.assertEqual({c.name for c in jar}, {"remixsid", "remixmid"})
        self.assertTrue(all(c.domain == ".vk.ru" and c.secure for c in jar))

    def test_download_checks_actual_bytes_not_the_reported_size(self):
        class Downloader:
            def __init__(self, options): self.options = options
            def __enter__(self): return self
            def __exit__(self, *_): pass
            def extract_info(self, *_args, **_kwargs): return {"filesize": 1}
            def process_ie_result(self, *_args, **_kwargs):
                self.options["progress_hooks"][0]({"downloaded_bytes": 33, "total_bytes": 1})

        with tempfile.TemporaryDirectory() as directory, patch.dict(sys.modules, {"yt_dlp": Obj(YoutubeDL=Downloader)}), patch.object(worker, "MAX_AUDIO_BYTES", 32):
            with self.assertRaises(worker.ImportFailure):
                worker.download(worker.song("id", "Title", "Artist", url="https://example.invalid/audio"), {"service": "vk"}, Path(directory))
            self.assertEqual(list(Path(directory).iterdir()), [])

    def test_conversion_cannot_publish_a_file_larger_than_its_limit(self):
        class Downloader:
            def __init__(self, options): self.options = options
            def __enter__(self): return self
            def __exit__(self, *_): pass
            def extract_info(self, *_args, **_kwargs): return {"filesize": 1}
            def process_ie_result(self, *_args, **_kwargs):
                Path(self.options["outtmpl"].replace("%(ext)s", "flac")).write_bytes(b"x" * 33)

        with tempfile.TemporaryDirectory() as directory, patch.dict(sys.modules, {"yt_dlp": Obj(YoutubeDL=Downloader)}), patch.object(worker, "MAX_AUDIO_BYTES", 32):
            with self.assertRaises(worker.ImportFailure):
                worker.download(worker.song("id", "Title", "Artist", url="https://example.invalid/audio"), {"service": "vk"}, Path(directory))
            self.assertEqual(list(Path(directory).iterdir()), [])

    @unittest.skipUnless(os.environ.get("HOPEA_TEST_DOWNLOAD"), "requires FFmpeg and yt-dlp")
    def test_real_audio_download_and_tags(self):
        import functools
        import http.server
        import threading
        fixtures = SOURCE.parents[2] / "node/tests/fixtures"
        audio = next(fixtures.glob("*.mp3"))

        class Handler(http.server.SimpleHTTPRequestHandler):
            def log_message(self, *_):
                pass

        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), functools.partial(Handler, directory=fixtures))
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            with tempfile.TemporaryDirectory() as directory:
                track = worker.song("local-fixture", "Import Test", "Test Artist",
                                    url=f"http://127.0.0.1:{server.server_port}/{audio.name}",
                                    album="Test Album", album_artist="Test Artist", track_number=2)
                path = worker.download(track, {"service": "vk"}, Path(directory))
                self.assertTrue(path.is_file())
                self.assertEqual(path.read_bytes()[:4], b"fLaC")
                probe = subprocess.run(["ffprobe", "-v", "error", "-show_entries", "format_tags",
                                        "-of", "json", str(path)], capture_output=True, text=True, check=True)
                tags = {k.lower(): v for k, v in json.loads(probe.stdout)["format"]["tags"].items()}
                self.assertEqual(tags["title"], "Import Test")
                self.assertEqual(tags["artist"], "Test Artist")
                self.assertEqual(tags["album"], "Test Album")
                self.assertEqual(tags["track"], "2")
        finally:
            server.shutdown()
            server.server_close()
            thread.join()


if __name__ == "__main__":
    unittest.main()
