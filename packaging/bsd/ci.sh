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
# The VM action mounts /home/runner/work, including the checkout's parent.
# pip builds independent Rust projects such as maturin: they must be outside
# both the vendored source and the outer checkout's Cargo workspace/config.
tools_root=$(mktemp -d "$(dirname "$(pwd)")/hopea-bsd-tools.XXXXXXXX")
trap 'rm -rf "$tools_root"' EXIT
mkdir -p "$tools_root/tmp" "$tools_root/cargo-home" "$tools_root/cache"
export TMPDIR="$tools_root/tmp"
export CARGO_HOME="$tools_root/cargo-home"
export XDG_CACHE_HOME="$tools_root/cache"
# NFS maps guest root to the host runner UID; pip rejects that cache owner.
# Prepared wheels are retained explicitly by native.sh for offline installation.
export PIP_NO_CACHE_DIR=1
cd ".packaging/bsd-source/hopea-${HOPEA_VERSION:?Set release version}"
test -f .cargo/config.toml
test -d vendor-registry
df -h . "$TMPDIR"
bash -o pipefail -c 'bash -x packaging/bsd/build.sh 2>&1 | tee "$HOPEA_DIST_DIR/bsd-build.log"'
