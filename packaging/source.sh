#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail
cd "$(dirname "$0")/.."
version=$(sed -n "s/.*version: '\([^']*\)'.*/\1/p" meson.build | head -1)
[[ $version =~ ^[0-9]+(\.[0-9]+)+$ ]] || exit 2
if [[ ${GITHUB_REF_TYPE:-} == tag && ${GITHUB_REF_NAME:-} != "v$version" ]]; then
  echo 'Tag must match the Meson version (vVERSION).' >&2; exit 2
fi
bash build-aux/make-dist.sh
stage=$(mktemp -d)
trap 'chmod -R u+w "$stage"; rm -rf "$stage"' EXIT
tar -xzf dist/hopea-src.tar.gz -C "$stage"
chmod -R u+w "$stage"
mv "$stage/hopea" "$stage/hopea-$version"
(
  cd "$stage/hopea-$version"
  mkdir -p .cargo
  cargo vendor --locked --versioned-dirs vendor-registry > .cargo/config.toml
)
epoch=${SOURCE_DATE_EPOCH:-$(git log -1 --format=%ct)}
tar --sort=name --mtime="@$epoch" --owner=0 --group=0 --numeric-owner \
  -czf "dist/hopea-$version-source.tar.gz" -C "$stage" "hopea-$version"
(cd dist && sha256sum "hopea-$version-source.tar.gz" > "hopea-$version-source.tar.gz.sha256")
echo "Prepared source archive for Hopea $version"
