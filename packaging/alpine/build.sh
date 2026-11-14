#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail
[[ ${HOPEA_CI_CONTAINER:-} == 1 ]] || exit 2
archive=${1:?Pass source archive}
apk add --no-cache alpine-sdk bash cargo rust meson ninja pkgconf blueprint-compiler \
  gettext-dev desktop-file-utils gtk4.0-dev libadwaita-dev libsoup3-dev openssl-dev \
  gstreamer-dev gst-plugins-base-dev gst-plugins-bad-dev python3-dev py3-pip libffi-dev
pkg-config --atleast-version=4.20 gtk4
pkg-config --atleast-version=1.8 libadwaita-1
root=$PWD
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
bash packaging/render.sh "$archive" "$work/recipes"
mkdir -p "$work/aports/community/hopea" "$work/cache" "$work/packages"
cp "$work/recipes/alpine/APKBUILD" "$work/aports/community/hopea/"
cp "$archive" "$work/cache/"
adduser -D hopeabuild
addgroup hopeabuild abuild
chown -R hopeabuild "$work"
chmod 755 "$work"
su -s /bin/sh -c 'abuild-keygen -an' hopeabuild
# This build key is public alongside the package; private key never leaves HOME.
# abuild now uses XDG config paths. Ask its own configuration loader instead of
# guessing ~/.abuild; this also supports older abuild releases.
# shellcheck disable=SC2016
public_key=$(su -s /bin/sh -c '. /usr/share/abuild/functions.sh; printf "%s.pub\n" "$PACKAGER_PRIVKEY"' hopeabuild)
[[ $public_key == /home/hopeabuild/* && $public_key == *.pub && -f $public_key ]] || { echo 'abuild did not create a public signing key.' >&2; exit 1; }
cp "$public_key" /etc/apk/keys/
export HOPEA_ALPINE_WORK="$work"
# shellcheck disable=SC2016
su -s /bin/sh -c 'cd "$HOPEA_ALPINE_WORK/aports/community/hopea" && SRCDEST="$HOPEA_ALPINE_WORK/cache" REPODEST="$HOPEA_ALPINE_WORK/packages" abuild -r -d' hopeabuild
mkdir -p "$root/dist"
find "$work/packages" -name '*.apk' -exec cp {} "$root/dist/" \;
cp "$public_key" "$root/dist/"
