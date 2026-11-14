#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail
cd "$(dirname "$0")/.."
for script in packaging/*.sh packaging/*/*.sh build-aux/make-dist.sh; do bash -n "$script"; done
for recipe in packaging/arch/PKGBUILD.in packaging/alpine/APKBUILD.in packaging/gentoo/*.ebuild*; do bash -n "$recipe"; done
sh -n build-aux/dist-vendor.sh
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
printf 'test source artifact\n' > "$work/hopea-1.2-source.tar.gz"
HOPEA_REPOSITORY=example/hopea HOPEA_VERSION=1.2 \
  bash packaging/render.sh "$work/hopea-1.2-source.tar.gz" "$work/recipes"
hash=$(sha256sum "$work/hopea-1.2-source.tar.gz" | cut -d ' ' -f1)
grep -Fq "$hash" "$work/recipes/arch/PKGBUILD"
hash512=$(sha512sum "$work/hopea-1.2-source.tar.gz" | cut -d ' ' -f1)
grep -Fq "$hash512" "$work/recipes/alpine/APKBUILD"
# Check the literal makepkg variable, not the current shell environment.
# shellcheck disable=SC2016
grep -Fq 'https://github.com/example/hopea/releases/download/v${pkgver}' "$work/recipes/arch/PKGBUILD"
test -x "$work/recipes/debian/rules"
if grep -RE '@(VERSION|REPOSITORY|SOURCE_SHA256|SOURCE_SHA512)@' "$work/recipes"; then exit 1; fi
# Release identifiers cannot inject shell/JSON/template content.
if HOPEA_REPOSITORY='bad/repo|oops' HOPEA_VERSION=1.2 \
  bash packaging/render.sh "$work/hopea-1.2-source.tar.gz" "$work/invalid"; then exit 1; fi
if HOPEA_REPOSITORY=example/hopea HOPEA_VERSION='../1.2' \
  bash packaging/render.sh "$work/hopea-1.2-source.tar.gz" "$work/invalid"; then exit 1; fi
if HOPEA_REPOSITORY=example/hopea HOPEA_VERSION=1.3 \
  bash packaging/render.sh "$work/hopea-1.2-source.tar.gz" "$work/invalid"; then exit 1; fi
# Packaging helpers cannot accidentally install into the host OS.
if DESTDIR=/ bash packaging/native.sh install > "$work/guard.log" 2>&1; then exit 1; fi
if DESTDIR=// bash packaging/native.sh install > "$work/guard.log" 2>&1; then exit 1; fi
grep -q 'Refusing DESTDIR=/' "$work/guard.log"
if HOPEA_CI_CONTAINER=0 bash packaging/linux-deps.sh debian > "$work/guard.log" 2>&1; then exit 1; fi
for script in packaging/void/build.sh packaging/alpine/build.sh; do
  if HOPEA_CI_CONTAINER=0 bash "$script" > "$work/guard.log" 2>&1; then exit 1; fi
done
echo 'Packaging templates, release identifiers and installation guards: OK'
