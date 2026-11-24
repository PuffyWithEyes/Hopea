#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail
cd "$(dirname "$0")/.."
source packaging/alt-apt.sh
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/apt/sources.list.d"
printf '%s\n' 'rpm [alt] http://ftp.altlinux.org/pub/distributions/ALTLinux Sisyphus/x86_64 classic' > "$work/apt/sources.list.d/alt.list"
cp "$work/apt/sources.list.d/alt.list" "$work/original"
export test_log="$work/log" test_mode=fail_primary
timeout() {
  [[ $1 == 180 || $1 == 1800 ]] || return 1
  shift
  "$@"
}
apt-get() {
  local sources=${2#Dir::Etc::sourcelist=}
  [[ $3 == -o && $4 == Dir::Etc::sourceparts=- ]] || return 1
  [[ $5 == -o && $6 == Acquire::http::Timeout=30 ]] || return 1
  local command=$7
  local line
  line=$(cat "$sources")
  [[ $line == 'rpm [alt] '* && $line == *' Sisyphus/x86_64 classic' ]] || return 1
  echo "$command $line" >> "$test_log"
  case $test_mode in
    fail_all) return 1 ;;
    fail_primary) [[ $line == *mirror.yandex.ru* ]] ;;
    fail_install) [[ $command == update || $line == *mirror.yandex.ru* ]] ;;
    succeed) return 0 ;;
  esac
}
alt_install_dependencies "$work/apt" ca-certificates
[[ $(wc -l < "$test_log") == 4 ]]
[[ $(grep -c '^install ' "$test_log") == 1 ]]
cmp "$work/original" "$work/apt/sources.list.d/alt.list"
: > "$test_log"
export test_mode=fail_install
alt_install_dependencies "$work/apt" ca-certificates
[[ $(wc -l < "$test_log") == 6 ]]
[[ $(grep -c '^install ' "$test_log") == 3 ]]
: > "$test_log"
export test_mode=fail_all
if alt_install_dependencies "$work/apt" ca-certificates; then exit 1; fi
[[ $(wc -l < "$test_log") == 4 ]]
if grep -q '^install ' "$test_log"; then exit 1; fi
: > "$test_log"
export test_mode=succeed
alt_install_dependencies "$work/apt" ca-certificates
[[ $(wc -l < "$test_log") == 2 ]]
echo 'ALT mirror fallback, retry bounds, authentication and failure checks: OK'
