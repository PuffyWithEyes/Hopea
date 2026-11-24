# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
"""Android pipe protocol with fake providers: never reads accounts or a phone."""
import importlib.util
import json
from pathlib import Path
import socket
import sys
import tempfile
import threading
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, ROOT / path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    sys.modules[name] = module
    return module


adapter = load("android_import", "build-aux/android/android_import.py")
auth = load("service_auth", "src/library/service_auth.py")
service = load("service_import", "src/library/service_import.py")


class AndroidImportTest(unittest.TestCase):
    def exchange(self, request, mode, commands=()):
        local, remote = socket.socketpair()
        errors = []
        def worker():
            try:
                adapter.run(remote.detach(), mode)
            except Exception as error:
                errors.append(type(error).__name__)
        thread = threading.Thread(target=worker)
        thread.start()
        local.settimeout(3)
        events = []
        with local, local.makefile("rwb") as stream:
            stream.write(json.dumps(request).encode() + b"\n")
            for command in commands:
                stream.write(json.dumps(command).encode() + b"\n")
            stream.flush()
            while line := stream.readline():
                events.append(json.loads(line))
        thread.join(3)
        self.assertFalse(thread.is_alive())
        self.assertEqual(errors, [])
        return events

    def test_authorization_returns_prompt_then_credentials(self):
        def sign_in(client_id, emit):
            self.assertEqual(client_id, "public-id")
            emit(dict(event="auth_prompt", url="https://accounts.spotify.com/authorize", user_code=""))
            return "fixture-token"
        with patch.object(auth, "spotify", sign_in):
            events = self.exchange(dict(service="spotify", client_id="public-id"), 1)
        self.assertEqual([e["event"] for e in events], ["auth_prompt", "credentials"])
        self.assertEqual(events[-1]["secret"], "fixture-token")

    def test_catalog_selection_uses_mobile_downloader_without_ffmpeg(self):
        track = service.song("one", "A song", "Artist", album="Album")
        with tempfile.TemporaryDirectory() as root:
            target = Path(root) / "chosen.source"
            with patch.dict(service.PROVIDERS, vk=lambda request: iter([track])), \
                 patch.object(adapter, "download", return_value=target) as download:
                events = self.exchange(dict(service="vk", secret="fixture", account="123", download=True,
                    covers=False, directory=root), 0, [dict(key="one"), dict(finish=True)])
            download.assert_called_once()
            self.assertEqual([e["event"] for e in events], ["catalog_track", "ready", "downloaded"])
            self.assertEqual(events[-1]["path"], str(target))

    def test_provider_exception_does_not_expose_token(self):
        with patch.object(auth, "spotify", side_effect=RuntimeError("secret token inside URL")):
            events = self.exchange(dict(service="spotify", client_id="public-id"), 1)
        self.assertEqual(events[0]["event"], "error")
        self.assertNotIn("token inside", json.dumps(events))

    def test_android_cannot_read_desktop_browser_profiles(self):
        events = self.exchange(dict(service="vk", browser="chrome", secret="fixture"), 0)
        self.assertEqual(events[0]["event"], "error")


if __name__ == "__main__":
    unittest.main()
