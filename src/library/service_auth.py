# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
"""Disposable provider authorization adapter. JSON stdin/stdout; no token cache."""
import base64
import contextlib
import hashlib
from http.server import BaseHTTPRequestHandler, HTTPServer
import json
import logging
import re
import secrets
import sys
import time
from urllib.parse import parse_qs, urlencode, urlsplit

SCOPES = 'user-library-read playlist-read-private playlist-read-collaborative streaming'
REDIRECT_URI = 'http://127.0.0.1:43821/callback'
YANDEX_SCOPES = 'music:read music:content'


class AuthError(Exception):
    pass


def access_token(value):
    if not isinstance(value, str) or not value or len(value) > 16384 or any(c.isspace() for c in value):
        raise AuthError('The service returned an invalid access token.')
    return value


def callback_result(path, state, host):
    """Ignore unrelated requests; reject ambiguous or forged OAuth responses."""
    if len(path) > 8192 or host != '127.0.0.1:43821':
        return None
    url = urlsplit(path)
    if url.scheme or url.netloc or url.fragment or url.path != '/callback':
        return None
    try:
        query = parse_qs(url.query, keep_blank_values=True, max_num_fields=16)
    except ValueError:
        return None
    if any(len(values) != 1 for values in query.values()):
        return None
    received = query.get('state', [''])[0]
    if not received.isascii() or not secrets.compare_digest(received, state):
        return None
    if 'error' in query:
        if 'code' in query:
            return None
        return ('error', 'Spotify sign-in was declined. Try again when ready.')
    code = query.get('code', [''])[0]
    if not code or len(code) > 4096 or not code.isascii() or any(c.isspace() for c in code):
        return None
    return ('code', code)


def pkce_challenge(verifier):
    return base64.urlsafe_b64encode(hashlib.sha256(verifier.encode('ascii')).digest()).rstrip(b'=').decode('ascii')


def spotify(client_id, emit):
    import requests
    if not isinstance(client_id, str) or not re.fullmatch(r'[0-9a-fA-F]{32}', client_id):
        raise AuthError('Enter the public Client ID from your Spotify developer application, not its Client Secret.')
    state, verifier = secrets.token_urlsafe(32), secrets.token_urlsafe(64)
    result = []

    class Callback(BaseHTTPRequestHandler):
        def setup(self):
            self.request.settimeout(2)
            super().setup()

        def log_message(self, *_args):
            pass  # HTTP request paths contain the authorization code.

        def do_GET(self):
            reply = callback_result(self.path, state, self.headers.get('Host'))
            body = b'Return to Hopea to finish sign-in.' if reply else b'Invalid sign-in callback.'
            self.send_response(200 if reply else 400)
            self.send_header('Content-Type', 'text/plain; charset=utf-8')
            self.send_header('Content-Length', str(len(body)))
            self.send_header('Cache-Control', 'no-store')
            self.send_header('Referrer-Policy', 'no-referrer')
            self.send_header('Content-Security-Policy', "default-src 'none'")
            self.send_header('Connection', 'close')
            self.end_headers()
            self.wfile.write(body)
            if reply:
                result.append(reply)

    class Server(HTTPServer):
        allow_reuse_address = False

        def handle_error(self, *_args):
            pass

    try:
        server = Server(('127.0.0.1', 43821), Callback)
    except OSError:
        raise AuthError('The Spotify callback port is busy. Close the other sign-in attempt and retry.') from None
    with server:
        server.timeout = 0.5
        emit({'event': 'auth_prompt', 'url': 'https://accounts.spotify.com/authorize?' + urlencode({
            'client_id': client_id, 'response_type': 'code', 'redirect_uri': REDIRECT_URI,
            'scope': SCOPES, 'state': state, 'code_challenge_method': 'S256',
            'code_challenge': pkce_challenge(verifier),
        }), 'user_code': ''})
        deadline = time.monotonic() + 300
        while not result:
            if time.monotonic() >= deadline:
                raise AuthError('Spotify sign-in timed out. Please try again.')
            server.handle_request()
    kind, value = result[0]
    if kind == 'error':
        raise AuthError(value)
    try:
        with requests.post('https://accounts.spotify.com/api/token', data={
            'client_id': client_id, 'grant_type': 'authorization_code', 'code': value,
            'redirect_uri': REDIRECT_URI, 'code_verifier': verifier,
        }, timeout=30, allow_redirects=False, stream=True) as response:
            if response.status_code != 200:
                raise AuthError('Spotify could not finish sign-in. Check the Client ID, registered redirect URI and allowed users.')
            data = bytearray()
            for part in response.iter_content(4096):
                data.extend(part)
                if len(data) > 65536:
                    raise AuthError('Spotify returned an oversized sign-in response.')
            token = json.loads(data)
        if token.get('token_type', '').lower() != 'bearer':
            raise AuthError('Spotify returned an unsupported token type.')
        if not set(SCOPES.split()).issubset(set(token.get('scope', '').split())):
            raise AuthError('Spotify did not grant the requested library and streaming permissions.')
        return access_token(token.get('access_token'))
    except AuthError:
        raise
    except Exception:
        raise AuthError('Unable to complete Spotify sign-in. Check the connection and try again.') from None


def valid_yandex_confirmation(page, user_code):
    if (not isinstance(page, str) or len(page) > 2048
            or any(ord(c) <= 32 or ord(c) == 127 for c in page)
            or not isinstance(user_code, str)
            or not re.fullmatch(r'[A-Za-z0-9-]{1,64}', user_code)):
        return False
    try:
        url = urlsplit(page)
        # The live OAuth endpoint returns ya.ru/device, while the API docs
        # also describe the oauth.yandex.ru/com confirmation pages.
        return (url.scheme == 'https'
                and url.hostname in ('ya.ru', 'oauth.yandex.ru', 'oauth.yandex.com')
                and url.path in ('/device', '/device/')
                and url.username is None and url.password is None
                and url.port in (None, 443) and not url.query and not url.fragment)
    except ValueError:
        return False


def yandex_auth_client():
    from yandex_music import Client
    from yandex_music.utils.request import Request

    class MusicOnlyRequest(Request):
        def post(self, url, data=None, *args, **kwargs):
            # The pinned library's Device Flow omits scope, which grants every
            # permission registered for its client. Narrow the initial request
            # while retaining the library's polling and cancellation behavior.
            if url == 'https://oauth.yandex.ru/device/code':
                data = dict(data or {})
                data['scope'] = YANDEX_SCOPES
                data.pop('optional_scope', None)
            return super().post(url, data, *args, **kwargs)

    return Client(request=MusicOnlyRequest())


def yandex(emit):
    def show_code(code):
        if not valid_yandex_confirmation(code.verification_url, code.user_code):
            raise AuthError('Yandex returned an invalid confirmation page or code.')
        emit({'event': 'auth_prompt', 'url': code.verification_url, 'user_code': code.user_code})

    client = yandex_auth_client()
    if not hasattr(client, 'device_auth'):
        raise AuthError('Update the yandex-music import dependency to a version with Device Flow support, or paste a token manually.')
    try:
        token = client.device_auth(on_code=show_code, timeout=300, device_name='Hopea')
        return access_token(token.access_token)
    except AuthError:
        raise
    except Exception:
        raise AuthError('Yandex sign-in expired, was declined, or could not connect. Please try again.') from None


def main():
    output = sys.stdout

    def emit(event):
        output.write(json.dumps(event) + '\n')
        output.flush()

    logging.disable(logging.CRITICAL)
    try:
        raw = sys.stdin.buffer.readline(4097)
        if len(raw) > 4096:
            raise AuthError('The sign-in request is too large.')
        request = json.loads(raw)
        with contextlib.redirect_stdout(sys.stderr):
            if request.get('service') == 'spotify':
                token = spotify(request.get('client_id'), emit)
            elif request.get('service') == 'yandex':
                token = yandex(emit)
            else:
                raise AuthError('This service does not support this sign-in flow.')
        emit({'event': 'credentials', 'secret': token, 'account': ''})
    except ModuleNotFoundError:
        emit({'event': 'error', 'message': 'Install the service import dependencies before signing in.'})
        return 1
    except AuthError as error:
        emit({'event': 'error', 'message': str(error)})
        return 1
    except Exception:
        emit({'event': 'error', 'message': 'Unable to start service sign-in. Please try again.'})
        return 1
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
