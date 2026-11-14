#!/bin/sh
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
# Install the missing X11 base/development sets in the disposable CI guest.
set -eu
[ "${HOPEA_CI_CONTAINER:-}" = 1 ] && [ "$(uname -s)" = NetBSD ] || exit 2
release=$(uname -r)
arch=$(uname -m)
case "$release/$arch" in
  10.1/amd64) ;;
  *) echo "No verified X11 set checksums for NetBSD $release/$arch" >&2; exit 2 ;;
esac
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cd "$work"
for set in xbase xcomp; do
  case $set in
    xbase) expected=b6ac2195f91d7ae395ec2734c9ae2a4727014629fbdc94562531225e0d51e3b2705047e8440fcf75e1dacdcecb3cb980df7a82f838c12ad5274ba8ce95e49b72 ;;
    xcomp) expected=40ef5271cbfcc7fce1bdbbd12217521b79cc074b9b05831184e941dd40d4db867afae7684c179a512c327eeee6cd8d4fd7daff86b5da3bbdfcc763910271dc68 ;;
  esac
  ftp -o "$set.tar.xz" "https://archive.netbsd.org/pub/NetBSD-archive/NetBSD-$release/$arch/binary/sets/$set.tar.xz"
  actual=$(cksum -a SHA512 -q "$set.tar.xz")
  [ "$actual" = "$expected" ] || { echo "$set checksum mismatch" >&2; exit 1; }
  tar -xpf "$set.tar.xz" -C /
done
