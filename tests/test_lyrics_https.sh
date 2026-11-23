#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
export HOPEA_TLS_FIXTURES="$work/certs"
mkdir -p "$HOPEA_TLS_FIXTURES"
# Disposable local CA and localhost-only server identity. No production keys,
# external service calls, installed CA changes or device access are involved.
openssl req -x509 -newkey rsa:2048 -nodes -days 2 -subj '/CN=Hopea TLS Test CA' \
  -keyout "$work/ca.key" -out "$work/ca.pem" >/dev/null 2>&1
openssl req -newkey rsa:2048 -nodes -subj '/CN=localhost' \
  -keyout "$work/server.key" -out "$work/server.csr" >/dev/null 2>&1
cat > "$work/server.ext" <<'EOF'
basicConstraints=critical,CA:FALSE
keyUsage=critical,digitalSignature,keyEncipherment
extendedKeyUsage=serverAuth
subjectAltName=DNS:localhost
EOF
openssl x509 -req -in "$work/server.csr" -CA "$work/ca.pem" -CAkey "$work/ca.key" \
  -CAcreateserial -days 2 -extfile "$work/server.ext" -out "$work/server.pem" >/dev/null 2>&1
openssl x509 -in "$work/ca.pem" -outform DER -out "$HOPEA_TLS_FIXTURES/ca.der"
openssl x509 -in "$work/server.pem" -outform DER -out "$HOPEA_TLS_FIXTURES/server.der"
openssl pkcs8 -topk8 -nocrypt -in "$work/server.key" -outform DER -out "$HOPEA_TLS_FIXTURES/server-key.der"
# Compile the exact Android transport on the host, without GTK or an emulator.
# Reuse the application's resolved dependency versions in the temporary crate.
cp "$root/Cargo.lock" "$work/Cargo.lock"
python3 - "$root" "$work" <<'PY'
import json
from pathlib import Path
import sys
import tomllib

root, work = map(Path, sys.argv[1:])
manifest = tomllib.loads((root / 'Cargo.toml').read_text())
android = manifest['target']['cfg(target_os = "android")']['dependencies']
versions = {p['name']: p['version'] for p in tomllib.loads((root / 'Cargo.lock').read_text())['package']}
(work / 'Cargo.toml').write_text(f'''[package]
name = "hopea-lyrics-https-test"
version = "0.1.0"
edition = "2021"
[lib]
path = {json.dumps(str(root / 'src/lyrics_https.rs'))}
[dependencies]
reqwest = {{ version = "={android['reqwest']['version']}", default-features = false, features = ["rustls"] }}
webpki-root-certs = "={android['webpki-root-certs']}"
tokio = {{ version = "={manifest['dependencies']['tokio']['version']}", features = ["rt", "net", "time"] }}
[dev-dependencies]
rustls = "={versions['rustls']}"
[workspace]
''')
PY
cargo test --manifest-path "$work/Cargo.toml" --target-dir "$root/target/lyrics-https-test" -- --test-threads=1
