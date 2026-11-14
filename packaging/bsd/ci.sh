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
source_dir="$(pwd)/.packaging/bsd-source/hopea-${HOPEA_VERSION:?Set release version}"
if [ "$(uname -s)" = NetBSD ]; then
  # The NetBSD image has a large local disk. Its clock drifts under QEMU even
  # after NTP sync; NFS stamps newly created files with the host's time, which
  # breaks Meson's clock-skew check. Keep every build input/output local.
  # /tmp is a small tmpfs in this image, so use /var/tmp on the root disk.
  tools_root=$(mktemp -d /var/tmp/hopea-bsd-tools.XXXXXXXX)
else
  # The VM action mounts /home/runner/work, including the checkout's parent.
  tools_root=$(mktemp -d "$(dirname "$(pwd)")/hopea-bsd-tools.XXXXXXXX")
fi
trap 'rm -rf "$tools_root"' EXIT
if [ "$(uname -s)" = NetBSD ]; then
  # Do not preserve NFS mtimes. Logs and final packages still go to dist on
  # the shared workspace for actions/upload-artifact.
  cp -R "$source_dir" "$tools_root/source"
  source_dir="$tools_root/source"
fi
# pip builds independent Rust projects such as maturin: temporary directories
# must be outside the source Cargo workspace and its vendored configuration.
mkdir -p "$tools_root/tmp" "$tools_root/cargo-home" "$tools_root/cache"
export TMPDIR="$tools_root/tmp"
export CARGO_HOME="$tools_root/cargo-home"
export XDG_CACHE_HOME="$tools_root/cache"
# NFS maps guest root to the host runner UID; pip rejects that cache owner.
# Prepared wheels are retained explicitly by native.sh for offline installation.
export PIP_NO_CACHE_DIR=1
cd "$source_dir"
test -f .cargo/config.toml
test -d vendor-registry
df -h . "$TMPDIR"
bash -o pipefail -c 'bash -x packaging/bsd/build.sh 2>&1 | tee "$HOPEA_DIST_DIR/bsd-build.log"'
