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
    # Base NetBSD provides pkg_add; pkgin is a separately installed frontend.
    if ! command -v pkgin >/dev/null 2>&1; then
      pkg_add pkgin
    fi
    # Reuse the release-matched mirrors selected by the VM's startup hook.
    repositories=${PKG_PATH:-$(sed -n 's/^PKG_PATH=//p' /etc/pkg_install.conf)}
    [ -n "$repositories" ] || { echo 'No NetBSD package repository configured' >&2; exit 1; }
    mkdir -p /usr/pkg/etc/pkgin
    printf '%s\n' "$repositories" | tr ';' '\n' > /usr/pkg/etc/pkgin/repositories.conf
    pkgin -y update
    pkgin -y upgrade
    pkgin -y install bash rust meson ninja-build pkgconf blueprint-compiler gtk4 \
      libadwaita libsoup3 gstreamer1 gst-plugins1-base gst-plugins1-bad \
      ffmpeg7 python313 py313-pip gettext-tools coreutils jq
    ;;
  *) exit 2;;
esac
