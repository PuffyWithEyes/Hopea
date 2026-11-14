#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail
platform=${1:?Specify the distribution}
archive=${2:?Specify the source archive}
: "${HOPEA_REPOSITORY:?Set the release repository}"
root=$PWD
version=${HOPEA_VERSION:?Set the release version}
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
tar -xzf "$archive" -C "$work"
source_dir="$work/hopea-$version"
cd "$source_dir"
bash packaging/render.sh "$root/$archive" "$work/recipes"
case $platform in
  arch)
    # makepkg intentionally refuses root. Only the temporary build tree is owned by the builder.
    useradd -m hopeabuild
    cp "$work/recipes/arch/PKGBUILD" "$work/PKGBUILD"
    cp "$root/$archive" "$work/"
    chown -R hopeabuild "$work"
    chmod 755 "$work"
    # Expanded inside the builder shell.
    # shellcheck disable=SC2016
    runuser -u hopeabuild -- bash -c 'cd "$1" && makepkg --noconfirm --nodeps' _ "$work"
    mkdir -p "$root/dist"
    cp "$work"/*.pkg.tar.* "$root/dist/"
    ;;
  debian|ubuntu)
    cp -R "$work/recipes/debian" debian
    bash packaging/native.sh prepare
    dpkg-buildpackage -us -uc -b
    mkdir -p "$root/dist"
    for file in "$work"/*.deb; do cp "$file" "$root/dist/$(basename "${file%.deb}")-$platform.deb"; done
    ;;
  fedora|opensuse|alt)
    bash packaging/native.sh prepare
    top="$work/rpmbuild"
    mkdir -p "$top"/{BUILD,BUILDROOT,RPMS,SOURCES,SPECS,SRPMS}
    cp "$root/$archive" "$top/SOURCES/"
    tar -czf "$top/SOURCES/hopea-import-wheels.tar.gz" -C .packaging wheels
    case $platform in
      fedora) runtime='Requires: ffmpeg, gstreamer1-plugins-good, gstreamer1-plugins-bad-free' ;;
      opensuse) runtime='Requires: ffmpeg, gstreamer-plugins-good, gstreamer-plugins-bad, gstreamer-plugins-libav' ;;
      alt) runtime='Requires: ffmpeg, gst-plugins-good1.0, gst-plugins-bad1.0, gst-libav, python3-modules-sqlite3' ;;
    esac
    sed "s|@RPM_REQUIRES@|$runtime|" "$work/recipes/rpm/hopea.spec" > "$top/SPECS/hopea.spec"
    # The container bootstrap checks pkg-config; distro package names differ.
    rpmbuild -bb --nodeps --define "_topdir $top" "$top/SPECS/hopea.spec"
    mkdir -p "$root/dist"
    while IFS= read -r -d '' file; do
      cp "$file" "$root/dist/$(basename "${file%.rpm}")-$platform.rpm"
    done < <(find "$top/RPMS" -name '*.rpm' -print0)
    ;;
  *) exit 2 ;;
esac
