#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
# Run from the source root. Downloads happen only in `prepare`, never install.
set -euo pipefail
build_dir=${HOPEA_BUILD_DIR:-.packaging/build}
prefix=${HOPEA_PREFIX:-/usr}
python=${HOPEA_BUILD_PYTHON:-python3}
# GNU-compatible checksum tool on Linux, macOS and BSD.
if command -v sha256sum >/dev/null; then checksum=sha256sum
elif command -v gsha256sum >/dev/null; then checksum=gsha256sum
else echo 'Install coreutils for SHA-256 verification.' >&2; exit 2; fi
case $prefix in /*) ;; *) echo 'HOPEA_PREFIX must be absolute' >&2; exit 2;; esac
case /$prefix/ in */../*) echo 'HOPEA_PREFIX cannot contain parent traversal' >&2; exit 2;; esac
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-4}
case ${1:-} in
  prepare)
    # Detect split/missing standard-library packages before compiling Rust.
    "$python" -I -c 'import ssl, sqlite3, venv'
    mkdir -p .packaging/wheels .cargo
    if [[ ! -f .cargo/config.toml ]]; then
      cargo vendor --locked --versioned-dirs vendor-registry > .cargo/config.toml
    fi
    "$python" -m pip wheel --disable-pip-version-check \
      --wheel-dir .packaging/wheels -r build-aux/import-requirements.txt
    (cd .packaging/wheels && "$checksum" ./*.whl > SHA256SUMS)
    ;;
  configure)
    pkg-config --atleast-version=4.20 gtk4
    pkg-config --atleast-version=1.8 libadwaita-1
    export CARGO_NET_OFFLINE=true
    meson setup "$build_dir" . --prefix="$prefix" --libdir=lib -Dprofile=default --buildtype=release
    ;;
  build)
    export CARGO_NET_OFFLINE=true
    meson compile -C "$build_dir" -j "$CARGO_BUILD_JOBS"
    meson compile -C "$build_dir" -j "$CARGO_BUILD_JOBS"
    ;;
  install)
    # Never install into a live system from a packaging job.
    case ${DESTDIR:-} in /*) ;; *) echo 'DESTDIR must be an absolute staging directory' >&2; exit 2;; esac
    mkdir -p "$DESTDIR"
    DESTDIR=$(cd "$DESTDIR" && pwd -P)
    [[ $DESTDIR != / && $DESTDIR != // ]] || { echo 'Refusing DESTDIR=/' >&2; exit 2; }
    meson install -C "$build_dir" --destdir "$DESTDIR" --no-rebuild
    (cd .packaging/wheels && "$checksum" -c SHA256SUMS)
    env_dir="$DESTDIR$prefix/lib/hopea-import"
    "$python" -m venv --without-pip "$env_dir"
    "$python" -m pip --python "$env_dir/bin/python3" install --no-compile \
      --no-index --find-links=.packaging/wheels -r build-aux/import-requirements.txt
    "$env_dir/bin/python3" -I -c 'import vk_api, bs4, yandex_music, ytmusicapi, yt_dlp, Cryptodome, spotipy, ssl, sqlite3'
    # Keep only the interpreter names used by the importer. Remove console
    # scripts and unused aliases, including Python 3.14's non-ASCII symlink.
    find "$env_dir/bin" \( -type f -o -type l \) ! -name 'python*' -delete
    find "$env_dir" -type d -name __pycache__ -prune -exec rm -rf {} +
    # venv records its creation command, including the temporary DESTDIR.
    # This provenance is not needed at runtime and fails RPM's check-buildroot.
    # Retain home/executable and isolation settings for the system interpreter.
    "$python" - "$env_dir/pyvenv.cfg" <<'PY'
from pathlib import Path
import sys

config = Path(sys.argv[1])
lines = config.read_text(encoding="utf-8").splitlines(keepends=True)
config.write_text(
    "".join(line for line in lines if line.partition("=")[0].strip() != "command"),
    encoding="utf-8",
)
PY
    # Private schema cache avoids overwriting the distro-wide gschemas.compiled.
    install -d "$DESTDIR$prefix/share/hopea/schemas" "$DESTDIR$prefix/libexec"
    cp data/io.hopea.Hopea.gschema.xml "$DESTDIR$prefix/share/hopea/schemas/"
    glib-compile-schemas "$DESTDIR$prefix/share/hopea/schemas"
    mv "$DESTDIR$prefix/bin/hopea" "$DESTDIR$prefix/libexec/hopea"
    cat > "$DESTDIR$prefix/bin/hopea" <<'LAUNCHER'
#!/bin/sh
root=$(CDPATH= cd "$(dirname "$0")/.." && pwd)
export GSETTINGS_SCHEMA_DIR="$root/share/hopea/schemas"
export HOPEA_IMPORT_PYTHON="$root/lib/hopea-import/bin/python3"
exec "$root/libexec/hopea" "$@"
LAUNCHER
    chmod 755 "$DESTDIR$prefix/bin/hopea"
    install -d "$DESTDIR$prefix/share/licenses/hopea"
    cp -R LICENSES ADDITIONAL-TERMS.md "$DESTDIR$prefix/share/licenses/hopea/"
    ;;
  *) echo 'Usage: packaging/native.sh prepare|configure|build|install' >&2; exit 2;;
esac
