#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
# Build the security-updated CPython with Chaquopy's upstream Android recipe.
set -euo pipefail
root=$(realpath -m "${1:?Runtime build directory required}")
abi=${2:?Android ABI required}
case "$abi" in arm64-v8a|armeabi-v7a|x86_64) ;; *) exit 2 ;; esac
version=3.11.18
release=$version-hopea1
revision=1f63c93d8beaf7062fbe51888ab9235475df3bb8
archive_sha=5f62711b12470fd435f83f1451c1b1ef4a179aebc715c716874b0967567d814e
artifact="$root/maven/com/chaquo/python/target/$release"
if [[ -f "$artifact/target-$release-$abi.zip" && -f "$artifact/target-$release-stdlib-pyc.zip" ]]; then
  exit 0
fi
if [[ -e "$artifact" ]]; then
  echo 'Use a separate Android runtime build directory for each ABI.' >&2
  exit 1
fi
mkdir -p "$root"
if [[ ! -d "$root/source/.git" ]]; then
  git init "$root/source"
  git -C "$root/source" remote add origin https://github.com/chaquo/chaquopy.git
  git -C "$root/source" fetch --depth 1 origin "$revision"
fi
git -C "$root/source" checkout --detach "$revision"
test "$(git -C "$root/source" rev-parse HEAD)" = "$revision"
source="$root/source"
mkdir -p "$source/target/python/build/$version"
archive="$source/target/python/build/$version/Python-$version.tgz"
if [[ ! -f "$archive" ]]; then
  curl --fail --location --proto '=https' --tlsv1.2 --retry 3 \
    "https://www.python.org/ftp/python/$version/Python-$version.tgz" -o "$archive.part"
  mv "$archive.part" "$archive"
fi
printf '%s  %s\n' "$archive_sha" "$archive" | sha256sum -c -
# Use the project's NDK and bounded parallelism. Do not let the upstream recipe
# re-download the source after its checksum was verified.
python3 - "$source" <<'PY'
from pathlib import Path
import sys
source = Path(sys.argv[1])
env = source / "target/android-env.sh"
text = env.read_text().replace("ndk_version=27.3.13750724", "ndk_version=28.2.13676358")
text = text.replace("export CPU_COUNT=$(nproc)", "export CPU_COUNT=${HOPEA_BUILD_JOBS:-4}")
env.write_text(text)
build = source / "target/python/build.sh"
text = build.read_text().replace(
    "wget -c https://www.python.org/ftp/python/$version_no_pre/$src_filename",
    'test -f "$src_filename"  # Checked by Hopea before invoking this recipe.')
build.write_text(text)
PY
bash "$source/target/python/build.sh" "$root/prefix/$abi" "$version"
mkdir -p "$root/licenses"
cp "$source/LICENSE.txt" "$root/licenses/Chaquopy.txt"
cp "$source/target/python/build/$version/$abi/LICENSE" "$root/licenses/CPython.txt"
cp "$source/target/python/build/$version/$abi/Modules/expat/COPYING" "$root/licenses/Expat.txt"
packaging=$(mktemp -d "$root/packaging.XXXXXXXX")
trap 'rm -rf "$packaging"' EXIT
bash "$source/target/package-target.sh" "$packaging/$release" "$root/prefix/$abi"
mkdir -p "$(dirname "$artifact")"
mv "$packaging/$release" "$artifact"
