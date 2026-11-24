#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later

# Source this helper in the disposable ALT container. Keep the image's signed
# repository definitions, changing only the official mirror URL on fallback.
alt_install_dependencies() (
  set -euo pipefail
  local apt_dir=$1
  shift
  local work source mirror attempt
  work=$(mktemp -d)
  trap 'rm -rf "$work"' EXIT
  for source in "$apt_dir/sources.list" "$apt_dir"/sources.list.d/*.list; do
    [[ -f $source ]] && cat "$source" >> "$work/original.list"
  done
  [[ -s $work/original.list ]] || { echo 'No ALT repository definitions found.' >&2; exit 1; }
  # HTTP also works before ca-certificates is installed in the minimal image.
  # The [alt] signature selector and APT authentication remain unchanged.
  for mirror in http://ftp.altlinux.org/pub/distributions/ALTLinux http://mirror.yandex.ru/altlinux; do
    sed "s|http://ftp.altlinux.org/pub/distributions/ALTLinux|$mirror|g" \
      "$work/original.list" > "$work/sources.list"
    local options=(-o "Dir::Etc::sourcelist=$work/sources.list"
      -o 'Dir::Etc::sourceparts=-' -o 'Acquire::http::Timeout=30')
    for attempt in 1 2; do
      echo "ALT dependencies: $mirror (attempt $attempt/2)" >&2
      if timeout 180 apt-get "${options[@]}" update && timeout 1800 apt-get "${options[@]}" install -y "$@"; then
        exit 0
      fi
      echo 'ALT repository operation failed; retrying with signed metadata.' >&2
    done
  done
  echo 'ALT dependencies unavailable from both mirrors; refusing to continue.' >&2
  exit 1
)
