#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
# Exercise the real staging/install path without compiling the GUI or using PyPI.
set -euo pipefail
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
python=${HOPEA_BUILD_PYTHON:-python3}
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/source/packaging" "$work/source/build-aux" "$work/source/.packaging/wheels" "$work/tools"
cp "${1:-$root/packaging/native.sh}" "$work/source/packaging/native.sh"
cp -R "$root/data" "$root/LICENSES" "$root/ADDITIONAL-TERMS.md" "$work/source/"
cd "$work/source"
printf 'hopea-install-fixture==1.0\n' > build-aux/import-requirements.txt
# A tiny local wheel stands in for service adapters; pip and venv remain real.
"$python" - <<'PY'
import zipfile

with zipfile.ZipFile('.packaging/wheels/hopea_install_fixture-1.0-py3-none-any.whl', 'w') as wheel:
    for name in ('vk_api', 'bs4', 'yandex_music', 'ytmusicapi', 'yt_dlp', 'Cryptodome', 'spotipy'):
        wheel.writestr(f'{name}.py', 'fixture = True\n')
    metadata = 'hopea_install_fixture-1.0.dist-info/'
    wheel.writestr(metadata + 'METADATA', 'Metadata-Version: 2.1\nName: hopea-install-fixture\nVersion: 1.0\n')
    wheel.writestr(metadata + 'WHEEL', 'Wheel-Version: 1.0\nRoot-Is-Purelib: true\nTag: py3-none-any\n')
    wheel.writestr(metadata + 'RECORD', '')
PY
(cd .packaging/wheels && sha256sum ./*.whl > SHA256SUMS)
# Only Meson's GUI installation is substituted; the real native script installs
# Python, validates imports, cleans staging paths and creates the final launcher.
cat > "$work/tools/meson" <<'SH'
#!/bin/sh
set -eu
test "$1" = install
mkdir -p "$DESTDIR/usr/bin"
cat > "$DESTDIR/usr/bin/hopea" <<'APP'
#!/bin/sh
exec "$HOPEA_IMPORT_PYTHON" -I -B -c 'import vk_api, bs4, yandex_music, ytmusicapi, yt_dlp, Cryptodome, spotipy, ssl, sqlite3; assert vk_api.fixture'
APP
chmod 755 "$DESTDIR/usr/bin/hopea"
SH
chmod +x "$work/tools/meson"
stage="$work/BUILDROOT with spaces"
PATH="$work/tools:$PATH" DESTDIR="$stage" HOPEA_PREFIX=/usr \
  HOPEA_BUILD_PYTHON="$python" bash packaging/native.sh install
# RPM rejects any installed text file containing the build root. Scan the private
# runtime before invoking Python again, so the check includes shipped files only.
"$python" - "$stage" <<'PY'
from pathlib import Path
import sys

stage = Path(sys.argv[1])
venv = stage / 'usr/lib/hopea-import'
for path in venv.rglob('*'):
    if path.is_file() and not path.is_symlink():
        assert str(stage).encode() not in path.read_bytes(), f'Build root leaked into {path}'
config = dict(line.split(' = ', 1) for line in (venv / 'pyvenv.cfg').read_text().splitlines() if ' = ' in line)
assert config['include-system-site-packages'] == 'false'
assert config['home'] and config['version']
PY
# Run Fedora's actual check against the original build root, before relocation.
if [[ -x /usr/lib/rpm/check-buildroot ]]; then
  RPM_BUILD_ROOT="$stage" /usr/lib/rpm/check-buildroot
fi
# The old path must disappear; imports must work through the installed launcher.
mv "$stage" "$work/installed"
"$work/installed/usr/bin/hopea"
echo 'Native install: no staging paths and relocated importer works: OK'
