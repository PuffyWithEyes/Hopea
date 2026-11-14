#!/bin/sh
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
set -eu
export PATH="/usr/local/sbin:/usr/local/bin:/usr/pkg/sbin:/usr/pkg/bin:/usr/pkg/gnu/bin:$PATH"
export HOPEA_CI_CONTAINER=1
mkdir -p dist
HOPEA_DIST_DIR="$(pwd)/dist"
export HOPEA_DIST_DIR
uname -a
df -h
if sh -x packaging/bsd/deps.sh >dist/bsd-dependencies.log 2>&1; then
  cat dist/bsd-dependencies.log
else
  status=$?
  cat dist/bsd-dependencies.log
  echo 'BSD dependency installation failed; see bsd-dependencies.log.' >&2
  exit "$status"
fi
# The host extracts the source artifact before mounting the workspace. Keep
# Cargo, pip and compilation off the guest's small root disk.
cd ".packaging/bsd-source/hopea-${HOPEA_VERSION:?Set release version}"
test -f .cargo/config.toml
test -d vendor-registry
mkdir -p .packaging/tmp .packaging/cargo-home .packaging/cache
source_root=$(pwd)
export TMPDIR="$source_root/.packaging/tmp"
export CARGO_HOME="$source_root/.packaging/cargo-home"
export XDG_CACHE_HOME="$source_root/.packaging/cache"
df -h . "$TMPDIR"
exec bash -o pipefail -c 'bash -x packaging/bsd/build.sh 2>&1 | tee "$HOPEA_DIST_DIR/bsd-build.log"'
