#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
# Run on the target BSD with native GTK/GStreamer, Rust, Python and pip installed.
set -euo pipefail
os=$(uname -s)
case $os in FreeBSD|DragonFly) prefix=/usr/local;; NetBSD) prefix=/usr/pkg;; OpenBSD) prefix=/usr/local;; *) exit 2;; esac
export HOPEA_PREFIX=$prefix
if [[ -z ${HOPEA_BUILD_PYTHON:-} ]]; then
  for candidate in python3 python3.14 python3.13 python3.12 python3.11; do
    if command -v "$candidate" >/dev/null && "$candidate" -I -c 'import pip, ssl, sqlite3, venv' 2>/dev/null; then
      HOPEA_BUILD_PYTHON=$candidate
      break
    fi
  done
fi
export HOPEA_BUILD_PYTHON=${HOPEA_BUILD_PYTHON:?Python 3 is required}
"$HOPEA_BUILD_PYTHON" -I -c 'import pip, ssl, sqlite3, venv'
"$HOPEA_BUILD_PYTHON" packaging/bsd/check-locks.py
version=${HOPEA_VERSION:?Set release version}
[[ $version =~ ^[0-9]+(\.[0-9]+)+$ ]] || exit 2
# Keep the install image on the guest filesystem: NFS maps root to the runner
# user, which would otherwise leak that user's ownership into BSD packages.
if [[ ${HOPEA_CI_CONTAINER:-} == 1 ]]; then
  stage=$(mktemp -d /tmp/hopea-install.XXXXXXXX)
else
  stage=$(mktemp -d)
fi
trap 'rm -rf "$stage"' EXIT
for requirement in 'gtk4 4.20' 'libadwaita-1 1.8'; do
  read -r library minimum <<< "$requirement"
  installed=$(pkg-config --modversion "$library" 2>/dev/null || printf 'missing')
  if ! pkg-config --atleast-version="$minimum" "$library"; then
    echo "$os: $library >= $minimum is required; installed: $installed. Update the package repository or build newer ports." >&2
    exit 1
  fi
done
bash packaging/native.sh prepare
bash packaging/native.sh configure
bash packaging/native.sh build
DESTDIR="$stage/root" bash packaging/native.sh install
dist=${HOPEA_DIST_DIR:-dist}
mkdir -p "$dist"
arch=$(uname -m)
tar -czf "$dist/hopea-$version-$os-$arch.tar.gz" -C "$stage/root" .
if [[ $os == FreeBSD || $os == DragonFly ]]; then
  # Record exact installed dependency versions/origins for this native package.
  command -v jq >/dev/null
  deps='{}'
  for dependency in gtk4 libadwaita libsoup3 gstreamer1 gstreamer1-plugins \
      gstreamer1-plugins-good gstreamer1-plugins-bad ffmpeg adwaita-icon-theme; do
    # Some ports name GTK4 gtk4, others gtk40: fail rather than emit missing metadata.
    record=$(pkg query -g '%n|%v|%o' "$dependency*")
    while IFS='|' read -r name dep_version origin; do
      [[ -n $name && -n $origin ]] || exit 1
      deps=$(jq -c --arg n "$name" --arg v "$dep_version" --arg o "$origin" \
        '. + {($n): {version: $v, origin: $o}}' <<< "$deps")
    done <<< "$record"
  done
  # OpenSSL may come from the base OS and then has no ports-package identity.
  # Record a ports OpenSSL when present; other required ports remain mandatory.
  if pkg info -e -g 'openssl*'; then
    record=$(pkg query -g '%n|%v|%o' 'openssl*')
    while IFS='|' read -r name dep_version origin; do
      [[ -n $name && -n $origin ]] || exit 1
      deps=$(jq -c --arg n "$name" --arg v "$dep_version" --arg o "$origin" \
        '. + {($n): {version: $v, origin: $o}}' <<< "$deps")
    done <<< "$record"
  fi
  py_record=$(pkg which -q "$(command -v "$HOPEA_BUILD_PYTHON")")
  # SQLite is a separate Python port and must also be installed by pkg users.
  sqlite_record=$(pkg which -q "$("$HOPEA_BUILD_PYTHON" -I -c 'import _sqlite3; print(_sqlite3.__file__)')")
  python_dependencies=$(pkg query '%n|%v|%o' "$py_record" "$sqlite_record")
  while IFS='|' read -r name dep_version origin; do
    deps=$(jq -c --arg n "$name" --arg v "$dep_version" --arg o "$origin" \
      '. + {($n): {version: $v, origin: $o}}' <<< "$deps")
  done <<< "$python_dependencies"
  jq -n --arg version "$version" --arg prefix "$prefix" --argjson deps "$deps" \
    '{name:"hopea",version:$version,origin:"audio/hopea",comment:"P2P music player",
      desc:"Hopea GTK4 music player",maintainer:"maintainers@localhost",www:"https://github.com",
      prefix:$prefix,licenselogic:"single",licenses:["GPLv3+"],deps:$deps}' > "$stage/+MANIFEST"
  (cd "$stage/root$prefix" && find . \( -type f -o -type l \) | sed 's|^./||' | sort) > "$stage/plist"
  package_format=()
  # DragonFly's system libarchive may lack built-in zstd support. pkg treats
  # that as an error, so explicitly use its supported xz compression there.
  if [[ $os == DragonFly ]]; then package_format=(-f txz); fi
  pkg create "${package_format[@]}" -m "$stage" -r "$stage/root" -p "$stage/plist" -o "$dist"
fi
if [[ $os == NetBSD || $os == OpenBSD ]]; then
  printf '%s\n' 'Hopea music player with peer-to-peer sharing.' > "$stage/description"
  (cd "$stage/root$prefix" && find . \( -type f -o -type l \) | sed 's|^./||' | sort) > "$stage/plist"
  # Read package identities from this builder, never invent dependency versions.
  pkg_info -a -I | awk '{print $1}' | \
    grep -E '^(gtk4|gtk\+4|libadwaita|libsoup3|gstreamer1|gst-plugins1[^-]*|ffmpeg[0-9]*|python3?[0-9]*|adwaita-icon-theme|openssl)-' \
    > "$stage/dependencies"
  test -s "$stage/dependencies"
  if [[ $os == NetBSD ]]; then
    printf 'OPSYS=NetBSD\nOS_VERSION=%s\nMACHINE_ARCH=%s\nPKGPATH=audio/hopea\n' \
      "$(uname -r)" "$arch" > "$stage/build-info"
    deps=$(tr '\n' ' ' < "$stage/dependencies")
    pkg_create -p "$stage/root$prefix" -I "$prefix" -B "$stage/build-info" \
      -P "$deps" -c '-Hopea music player' -d "$stage/description" \
      -f "$stage/plist" "$dist/hopea-$version.tgz"
  else
    # OpenBSD records executable/shared-object types and checks library reachability.
    awk '/^(bin\/hopea-spotify|libexec\/hopea)$/ { print "@bin " $0; next }
         /\.so([.][0-9]+)*$/ { print "@so " $0; next } { print }' \
      "$stage/plist" > "$stage/openbsd-plist"
    dependency_args=()
    while IFS= read -r name; do
      pkgpath=$(pkg_info -q -P "$name")
      [[ -n $pkgpath && $pkgpath != *$'\n'* ]] || exit 1
      dependency_args+=(-P "$pkgpath:=:$name")
    done < "$stage/dependencies"
    pkg_create -B "$stage/root" -p "$prefix" -A "$arch" \
      -D COMMENT='Hopea music player' -D FULLPKGPATH=audio/hopea -D PORTSDIR=/usr/ports \
      -d "$stage/description" -f "$stage/openbsd-plist" "${dependency_args[@]}" \
      "$dist/hopea-$version.tgz"
  fi
  mv "$dist/hopea-$version.tgz" "$dist/hopea-$version-$os-$arch.tgz"
fi
