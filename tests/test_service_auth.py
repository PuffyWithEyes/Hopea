# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
"""Offline authorization tests: no real accounts, cookies or provider requests."""
import importlib.util
import io
import json
from pathlib import Path
import sys
from types import SimpleNamespace as Obj
import unittest
from unittest.mock import patch
from urllib.parse import parse_qs, urlencode, urlsplit

spec = importlib.util.spec_from_file_location('service_auth', Path(__file__).resolve().parents[1] / 'src/library/service_auth.py')
auth = importlib.util.module_from_spec(spec)
spec.loader.exec_module(auth)


class AuthTests(unittest.TestCase):
    def test_pkce_rfc7636_vector(self):
        self.assertEqual(auth.pkce_challenge('dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk'),
                         'E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM')

    def test_callback_state_host_path_and_ambiguity(self):
        host = '127.0.0.1:43821'
        good = '/callback?state=fixture&code=code'
        self.assertEqual(auth.callback_result(good, 'fixture', host), ('code', 'code'))
        for path in ['/callback?state=wrong&code=code', '/callback?state=fixture&state=other&code=code',
                     '/callback?state=fixture&code=a&code=b', '/callback?state=fixture&code=',
                     '/callback?state=fixture&code=a&error=denied', '/callback?state=fixture&code=%0A',
                     '/other?state=fixture&code=a', 'https://evil.test/callback?state=fixture&code=a',
                     '/callback?state=%FF&code=a', good + '#' + 'fragment', good + 'x' * 8192]:
            self.assertIsNone(auth.callback_result(path, 'fixture', host), path)
        self.assertIsNone(auth.callback_result(good, 'fixture', 'evil.test'))
        self.assertEqual(auth.callback_result('/callback?state=fixture&error=access_denied', 'fixture', host)[0], 'error')

    def test_spotify_exchange_uses_pkce_and_closes_listener(self):
        events, calls, closed = [], [], []

        class Server:
            def __init__(self, address, handler):
                self.handler = handler
                self.address = address
                self.attempts = 0
            def __enter__(self): return self
            def __exit__(self, *_): closed.append(True)
            def handle_request(self):
                query = parse_qs(urlsplit(events[0]['url']).query)
                handler = self.handler.__new__(self.handler)
                state = 'forged' if self.attempts == 0 else query['state'][0]
                self.attempts += 1
                handler.path = '/callback?' + urlencode({'state': state, 'code': 'fixture-code'})
                handler.headers = {'Host': '127.0.0.1:43821'}
                handler.wfile = io.BytesIO()
                handler.send_response = lambda status: calls.append(status)
                handler.send_header = lambda *_: None
                handler.end_headers = lambda: None
                handler.do_GET()
                self.assert_not_echoed(handler.wfile.getvalue())
            def assert_not_echoed(self, body):
                assert b'fixture-code' not in body

        class Response:
            status_code = 200
            def __enter__(self): return self
            def __exit__(self, *_): pass
            def iter_content(self, _size):
                yield json.dumps({'access_token': 'fixture-token', 'token_type': 'Bearer', 'scope': auth.SCOPES}).encode()

        def post(url, **kwargs):
            self.assertEqual(url, 'https://accounts.spotify.com/api/token')
            self.assertFalse(kwargs['allow_redirects'])
            self.assertEqual(kwargs['data']['code'], 'fixture-code')
            self.assertEqual(kwargs['data']['redirect_uri'], auth.REDIRECT_URI)
            query = parse_qs(urlsplit(events[0]['url']).query)
            self.assertEqual(query['code_challenge'][0], auth.pkce_challenge(kwargs['data']['code_verifier']))
            self.assertNotIn('client_secret', kwargs['data'])
            return Response()
        with patch.object(auth, 'HTTPServer', Server), patch.dict(sys.modules, requests=Obj(post=post)):
            self.assertEqual(auth.spotify('a' * 32, events.append), 'fixture-token')
        self.assertEqual(calls, [400, 200])
        self.assertEqual(closed, [True])
        self.assertNotIn('fixture-token', repr(events))

    def test_yandex_shows_only_user_code_and_returns_token(self):
        events = []
        class Client:
            def device_auth(self, **kwargs):
                self_kwargs = kwargs
                assert self_kwargs['timeout'] == 300
                self_kwargs['on_code'](Obj(verification_url='https://ya.ru/device', user_code='ABCD-EFGH', device_code='private-code'))
                return Obj(access_token='fixture-token')
        with patch.dict(sys.modules, yandex_music=Obj(Client=Client)):
            self.assertEqual(auth.yandex(events.append), 'fixture-token')
        self.assertEqual(events[0]['user_code'], 'ABCD-EFGH')
        self.assertEqual(events[0]['url'], 'https://ya.ru/device')
        self.assertNotIn('private-code', repr(events))
        self.assertNotIn('fixture-token', repr(events))

    def test_yandex_confirmation_pages_and_codes_are_validated(self):
        for page in ['https://ya.ru/device', 'https://oauth.yandex.ru/device',
                     'https://oauth.yandex.com/device', 'https://ya.ru:443/device/']:
            self.assertTrue(auth.valid_yandex_confirmation(page, 'abcd1234'), page)
        for page in ['http://ya.ru/device', 'https://ya.ru.evil.test/device',
                     'https://evil.test/device', 'https://ya.ru/other',
                     'https://ya.ru/device?redirect=https://evil.test', 'https://ya.ru/device#fragment',
                     'https://ya.ru:8443/device', 'https://evil.test@ya.ru/device', 'https://@ya.ru/device',
                     'https://ya.ru/de\nvice', 'https://ya.ru/device/extra', 'https://ya.ru:invalid/device',
                     'https://[broken/device', None]:
            self.assertFalse(auth.valid_yandex_confirmation(page, 'abcd1234'), page)
        for code in ['', 'a' * 65, 'ab\ncd', 'code with spaces', '<code>', None, 123]:
            self.assertFalse(auth.valid_yandex_confirmation('https://ya.ru/device', code))

    def test_yandex_rejects_foreign_pages_and_redacts_provider_errors(self):
        class Client:
            def device_auth(self, **kwargs):
                kwargs['on_code'](Obj(verification_url='https://evil.test/device', user_code='ABCD'))
        with patch.dict(sys.modules, yandex_music=Obj(Client=Client)):
            with self.assertRaises(auth.AuthError): auth.yandex(lambda event: self.fail('Foreign page emitted'))
        Client.device_auth = lambda *_args, **_kwargs: (_ for _ in ()).throw(ValueError('secret-token-in-provider-error'))
        with patch.dict(sys.modules, yandex_music=Obj(Client=Client)):
            with self.assertRaises(auth.AuthError) as failure: auth.yandex(lambda _: None)
        self.assertNotIn('secret-token', str(failure.exception))


if __name__ == '__main__':
    unittest.main()
