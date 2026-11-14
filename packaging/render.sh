#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail
# Explicit release identity: never substitute the upstream repository by accident.
repo=${HOPEA_REPOSITORY:?Set HOPEA_REPOSITORY to the release owner/repository}
version=${HOPEA_VERSION:?Set HOPEA_VERSION to the release version without v}
source_file=${1:?Pass the matching release source archive}
[[ $repo =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]] || exit 2
[[ $version =~ ^[0-9]+(\.[0-9]+)+$ ]] || exit 2
[[ $(basename "$source_file") == "hopea-$version-source.tar.gz" ]] || exit 2
hash=$(sha256sum "$source_file" | cut -d ' ' -f1)
hash512=$(sha512sum "$source_file" | cut -d ' ' -f1)
out=${2:-dist/recipes}
while IFS= read -r -d '' template; do
  relative=${template#packaging/}
  destination="$out/${relative%.in}"
  mkdir -p "$(dirname "$destination")"
  sed -e "s|@VERSION@|$version|g" -e "s|@REPOSITORY@|$repo|g" \
      -e "s|@SOURCE_SHA256@|$hash|g" -e "s|@SOURCE_SHA512@|$hash512|g" "$template" > "$destination"
done < <(find packaging -name '*.in' -type f -print0)
cp -R packaging/debian/. "$out/debian/"
# cp includes the template too; install the rendered changelog over it.
sed "s|@VERSION@|$version|g" packaging/debian/changelog.in > "$out/debian/changelog"
rm -f "$out/debian/changelog.in"
printf '%s\n' 'Review generated recipes before uploading to a distribution repository.'
