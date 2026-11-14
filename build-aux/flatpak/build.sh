#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail
cd "$(dirname "$0")/../.."
for tool in flatpak flatpak-builder appstreamcli flock; do
    command -v "$tool" >/dev/null || { echo "Missing build tool: $tool" >&2; exit 1; }
done
mkdir -p _flatpak dist
exec 9>_flatpak/build.lock
flock -n 9 || { echo 'Another Flatpak build is already running.' >&2; exit 1; }

# Install prerequisites only if missing. --install-deps-from otherwise updates
# the SDK and graphics extensions on every build, including NVIDIA downloads.
deps=()
for ref in org.gnome.Platform//51 org.gnome.Sdk//51 org.freedesktop.Sdk.Extension.rust-stable//26.08; do
    if ! flatpak info "$ref" >/dev/null 2>&1; then
        deps=(--install-deps-from=flathub)
        break
    fi
done
# /run/ccache is mounted persistently by flatpak-builder. The manifest keeps
# Cargo's registry and target there too, separate from C compiler cache entries.
flatpak-builder --user "${deps[@]}" --ccache --jobs=4 --force-clean --disable-rofiles-fuse \
    --state-dir=_flatpak/state --repo=_flatpak/repo \
    _flatpak/build build-aux/flatpak/io.hopea.Hopea.json
flatpak build-bundle _flatpak/repo dist/hopea.flatpak io.hopea.Hopea \
    --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo
(cd dist && sha256sum hopea.flatpak > hopea.flatpak.sha256)
echo 'Ready: dist/hopea.flatpak'
