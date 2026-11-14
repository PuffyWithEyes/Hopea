#!/bin/sh
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
# Disposable CI VM only. Native users can install equivalent ports themselves.
set -eu
export PATH="/usr/local/sbin:/usr/local/bin:/usr/pkg/sbin:/usr/pkg/bin:/usr/pkg/gnu/bin:$PATH"
[ "${HOPEA_CI_CONTAINER:-}" = 1 ] || exit 2
case "$(uname -s)" in
  FreeBSD|DragonFly)
    # pip follows the repository's default Python version, not a fixed py313.
    pkg update -f
    # CI images can contain libraries from an older repository snapshot.
    # Upgrade them together before adding GTK and its loader modules.
    pkg upgrade -y
    pip_package=$(pkg search -q -x '^py3[0-9]+-pip-[0-9]' | awk -F- '
      { version=$1; sub(/^py/, "", version); if (version+0 > newest) {newest=version+0; package=$0} }
      END { if (package != "") print package; else exit 1 }')
    pkg install -y bash rust meson ninja pkgconf blueprint-compiler gtk4 libadwaita \
      libsoup3 gstreamer1 gstreamer1-plugins gstreamer1-plugins-good \
      gstreamer1-plugins-bad ffmpeg "$pip_package" "${pip_package%%-*}-sqlite3" gettext-tools \
      desktop-file-utils coreutils jq
    pkg clean -ay
    ;;
  OpenBSD)
    pkg_add -I bash rust meson ninja pkgconf blueprint-compiler gtk4 libadwaita \
      libsoup3 gstreamer1 gstreamer1-plugins-base gstreamer1-plugins-bad \
      ffmpeg python%3.13 py3-pip gettext-tools coreutils jq
    ;;
  NetBSD)
    sh packaging/bsd/netbsd-x11.sh
    # Base NetBSD provides pkg_add; pkgin is a separately installed frontend.
    if ! command -v pkgin >/dev/null 2>&1; then
      pkg_add pkgin
    fi
    # Reuse the release-matched mirrors selected by the VM's startup hook.
    repositories=${PKG_PATH:-$(sed -n 's/^PKG_PATH=//p' /etc/pkg_install.conf)}
    [ -n "$repositories" ] || { echo 'No NetBSD package repository configured' >&2; exit 1; }
    mkdir -p /usr/pkg/etc/pkgin
    printf '%s\n' "$repositories" | tr ';' '\n' > /usr/pkg/etc/pkgin/repositories.conf
    run_pkgin() {
      if pkgin -y "$@"; then return 0; else status=$?; fi
      if [ -f /var/db/pkgin/pkg_install-err.log ]; then
        cat /var/db/pkgin/pkg_install-err.log >&2
      fi
      return "$status"
    }
    run_pkgin update
    run_pkgin upgrade
    run_pkgin install bash rust meson ninja-build pkgconf gtk4 \
      libadwaita libsoup3 gstreamer1 gst-plugins1-base gst-plugins1-bad \
      ffmpeg7 python313 py313-pip py313-gobject3 gettext-tools coreutils jq
    # Blueprint is absent from some binary pkgsrc repositories. Its universal
    # wheel uses PyGObject from pkgsrc; keep it out of the system Python tree.
    python3.13 -m venv --without-pip --system-site-packages /usr/local/lib/hopea-blueprint
    printf '%s\n' 'blueprint-compiler==0.22.2 --hash=sha256:f6107b34ca21daa209e8e26ce518269e2e332e674da6450ba882ca72d33d2f32' \
      > /usr/local/lib/hopea-blueprint/requirements.txt
    python3.13 -m pip --python /usr/local/lib/hopea-blueprint/bin/python3 install \
      --only-binary=:all: --no-deps --require-hashes \
      -r /usr/local/lib/hopea-blueprint/requirements.txt
    mkdir -p /usr/local/bin
    ln -sf /usr/local/lib/hopea-blueprint/bin/blueprint-compiler /usr/local/bin/blueprint-compiler
    /usr/local/bin/blueprint-compiler --version
    ;;
  *) exit 2;;
esac
