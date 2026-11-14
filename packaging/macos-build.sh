#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail
[[ $(uname -s) == Darwin ]] || exit 2
repo=${HOPEA_REPOSITORY:?Set release repository}
version=${HOPEA_VERSION:?Set release version}
[[ $repo =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ && $version =~ ^[0-9]+(\.[0-9]+)+$ ]] || exit 2
root=$PWD
brew install coreutils rust meson ninja pkgconf blueprint-compiler gettext gtk4 libadwaita libsoup \
  gstreamer openssl@3 ffmpeg python@3.14 adwaita-icon-theme
HOPEA_BUILD_PYTHON="$(brew --prefix python@3.14)/bin/python3.14"
export HOPEA_BUILD_PYTHON
PATH="$(brew --prefix coreutils)/libexec/gnubin:$(brew --prefix gettext)/bin:$PATH"
export PATH
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
tar -xzf "dist/hopea-$version-source.tar.gz" -C "$work"
cd "$work/hopea-$version"
bash packaging/native.sh prepare
# Include per-platform wheels in a separately named source artifact. It can be
# rebuilt offline by the generated formula on the same Python/architecture.
arch=$(uname -m)
prepared="hopea-$version-macos-$arch-source.tar.gz"
tar -czf "$root/dist/$prepared" -C "$work" "hopea-$version"
hash=$(shasum -a 256 "$root/dist/$prepared" | cut -d ' ' -f1)
tap="${repo%/*}/hopea-build"
brew tap-new "$tap"
formula="$(brew --repo "$tap")/Formula/hopea.rb"
sed -e "s|@VERSION@|$version|g" -e "s|@REPOSITORY@|$repo|g" \
    -e "s|@SOURCE_SHA256@|$hash|g" \
    -e "s|https://github.com/$repo/releases/download/v$version/hopea-$version-source.tar.gz|file://$root/dist/$prepared|" \
    packaging/homebrew/hopea.rb.in > "$formula"
brew install --verbose --build-bottle "$tap/hopea"
brew test "$tap/hopea"
(cd "$root/dist" && brew bottle --json --root-url="https://github.com/$repo/releases/download/v$version" "$tap/hopea")
# Publish a source-installable formula whose URL will exist in the same release.
sed "s|file://$root/dist/$prepared|https://github.com/$repo/releases/download/v$version/$prepared|" \
  "$formula" > "$root/dist/hopea-$arch.rb"
