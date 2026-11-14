#!/bin/sh
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
# Give the single FreeBSD builder local NFS locks without changing the action's
# original mount. The host only consumes artifacts after the guest finishes.
set -eu
[ "${HOPEA_CI_CONTAINER:-}" = 1 ] && [ "$(uname -s)" = FreeBSD ] || exit 2
shared_root=/home/runner/work
workspace=$(pwd -P)
case "$workspace" in
  "$shared_root"/*) relative=${workspace#"$shared_root"/} ;;
  *) echo "Workspace is outside the VM's shared directory: $workspace" >&2; exit 2 ;;
esac
remote=$(mount -p | awk -v path="$shared_root" '$2 == path && $3 == "nfs" {print $1}')
[ -n "$remote" ] || { echo 'Expected NFS workspace mount is missing' >&2; exit 1; }
mount_dir=$(mktemp -d /tmp/hopea-nfs.XXXXXXXX)
mounted=0
cleanup() {
  if [ "$mounted" = 1 ]; then umount "$mount_dir" || return; fi
  rmdir "$mount_dir"
}
trap cleanup EXIT
# FreeBSD ignores nolockd on mount updates; it must be set on a new mount.
# Local locks still serialize all Cargo/pip processes within this one VM.
mount_nfs -o tcp,nolockd "$remote" "$mount_dir"
mounted=1
(
  cd "$mount_dir/$relative"
  sh packaging/bsd/ci.sh
)
