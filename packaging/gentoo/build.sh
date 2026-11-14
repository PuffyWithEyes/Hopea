#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail
[[ ${HOPEA_CI_CONTAINER:-} == 1 ]] || exit 2
version=${HOPEA_VERSION:?Set release version}
archive=${1:?Pass source archive}
bash packaging/render.sh "$archive" .packaging/recipes
overlay=$PWD/.packaging/gentoo-overlay
mkdir -p "$overlay"/{media-sound/hopea,dev-python/vk-api,dev-python/yandex-music,dev-python/pycryptodomex,metadata,profiles}
cp .packaging/recipes/gentoo/hopea.ebuild "$overlay/media-sound/hopea/hopea-$version.ebuild"
cp packaging/gentoo/vk-api.ebuild "$overlay/dev-python/vk-api/vk-api-11.10.1.ebuild"
cp packaging/gentoo/yandex-music.ebuild "$overlay/dev-python/yandex-music/yandex-music-3.2.2.ebuild"
cp packaging/gentoo/pycryptodomex.ebuild "$overlay/dev-python/pycryptodomex/pycryptodomex-3.23.0.ebuild"
printf 'hopea\n' > "$overlay/profiles/repo_name"
printf 'masters = gentoo\nthin-manifests = true\n' > "$overlay/metadata/layout.conf"
emerge-webrsync
# These namespace features require privileges unavailable in Docker's default
# sandbox. Keep Portage's filesystem sandbox and Docker's isolation enabled.
export FEATURES="${FEATURES:-} -ipc-sandbox -network-sandbox -pid-sandbox"
mkdir -p /etc/portage/repos.conf /etc/portage/package.accept_keywords
printf '[hopea]\nlocation = %s\nmasters = gentoo\nauto-sync = no\n' "$overlay" > /etc/portage/repos.conf/hopea.conf
printf 'media-sound/hopea ~amd64\ndev-python/vk-api ~amd64\ndev-python/yandex-music ~amd64\ndev-python/pycryptodomex ~amd64\n' \
  > /etc/portage/package.accept_keywords/hopea
mkdir -p /var/cache/distfiles
cp "$archive" /var/cache/distfiles/
while IFS= read -r -d '' recipe; do ebuild "$recipe" manifest; done < <(find "$overlay" -name '*.ebuild' -print0)
mkdir -p /etc/portage/package.use
printf 'net-misc/curl http2 ssl\n' > /etc/portage/package.use/hopea-build
# Reconcile the stage3's installed packages with the freshly synced tree/USE.
emerge --update --deep --newuse --with-bdeps=y --usepkg=y --buildpkg=y media-sound/hopea
mkdir -p dist
find /var/cache/binpkgs -path '*/hopea*' -type f \( -name '*.gpkg.tar' -o -name '*.tbz2' \) -exec cp {} dist/ \;
