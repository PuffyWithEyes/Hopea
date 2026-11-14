#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail
version=${HOPEA_VERSION:?Set release version}
repo=${HOPEA_REPOSITORY:?Set release repository}
source_file="dist/hopea-$version-source.tar.gz"
bash packaging/render.sh "$source_file" dist/recipes
windows_file="dist/hopea-$version-windows-x86_64.zip"
[[ -f $windows_file ]] || { echo 'Missing Windows artifact; cannot generate Scoop hash.' >&2; exit 1; }
[[ -f "dist/hopea-$version-windows-x86_64-setup.exe" ]] || { echo 'Missing Windows installer.' >&2; exit 1; }
windows_hash=$(sha256sum "$windows_file" | cut -d ' ' -f1)
sed "s|@WINDOWS_SHA256@|$windows_hash|g" dist/recipes/scoop/hopea.json > dist/hopea.json
rm -rf dist/recipes/scoop dist/recipes/homebrew
# The generated platform formulae contain their own prepared-source checksums.
mkdir -p dist/recipes/homebrew
cp dist/hopea-*.rb dist/recipes/homebrew/
# Include ready-to-place Gentoo overlay files; Manifest generation is done by Portage.
mkdir -p dist/recipes/gentoo/{media-sound/hopea,dev-python/vk-api,dev-python/yandex-music,dev-python/pycryptodomex,metadata,profiles}
mv dist/recipes/gentoo/hopea.ebuild "dist/recipes/gentoo/media-sound/hopea/hopea-$version.ebuild"
cp packaging/gentoo/vk-api.ebuild dist/recipes/gentoo/dev-python/vk-api/vk-api-11.10.1.ebuild
cp packaging/gentoo/yandex-music.ebuild dist/recipes/gentoo/dev-python/yandex-music/yandex-music-3.2.2.ebuild
cp packaging/gentoo/pycryptodomex.ebuild dist/recipes/gentoo/dev-python/pycryptodomex/pycryptodomex-3.23.0.ebuild
printf 'hopea\n' > dist/recipes/gentoo/profiles/repo_name
printf 'masters = gentoo\nthin-manifests = true\n' > dist/recipes/gentoo/metadata/layout.conf
for platform in fedora opensuse alt; do
  case $platform in
    fedora) runtime='Requires: ffmpeg, gstreamer1-plugins-good, gstreamer1-plugins-bad-free' ;;
    opensuse) runtime='Requires: ffmpeg, gstreamer-plugins-good, gstreamer-plugins-bad, gstreamer-plugins-libav' ;;
    alt) runtime='Requires: ffmpeg, gst-plugins-good1.0, gst-plugins-bad1.0, gst-libav' ;;
  esac
  sed "s|@RPM_REQUIRES@|$runtime|" dist/recipes/rpm/hopea.spec > "dist/recipes/rpm/hopea-$platform.spec"
done
rm dist/recipes/rpm/hopea.spec
if grep -R -nE '@(VERSION|REPOSITORY|SOURCE_SHA256|SOURCE_SHA512|WINDOWS_SHA256|RPM_REQUIRES)@' dist/recipes dist/hopea.json; then
  echo 'Unresolved release placeholders' >&2; exit 1
fi
tar -czf "dist/hopea-$version-package-recipes.tar.gz" -C dist recipes
rm -rf dist/recipes
# Record only actual files, not directories; sorting keeps the manifest stable.
manifest=$(mktemp)
trap 'rm -f "$manifest"' EXIT
(cd dist && find . -maxdepth 1 -type f ! -name SHA256SUMS -print0 | sort -z | xargs -0 sha256sum) > "$manifest"
mv "$manifest" dist/SHA256SUMS
printf 'Prepared artifacts for %s v%s\n' "$repo" "$version"
