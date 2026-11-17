#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/bin"
cat > "$work/bin/pacman" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "$TEST_LOG"
if [[ $1 == -Sw ]]; then
    count=$(grep -c '^-Sw ' "$TEST_LOG")
    (( count > TEST_DOWNLOAD_FAILURES )) || exit 7
else
    exit "$TEST_INSTALL_STATUS"
fi
EOF
cat > "$work/bin/sleep" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$1" >> "$TEST_WAITS"
EOF
chmod +x "$work/bin/"*
export PATH="$work/bin:$PATH" TEST_LOG="$work/calls" TEST_WAITS="$work/waits"
export MSYSTEM=UCRT64 TEST_DOWNLOAD_FAILURES=0 TEST_INSTALL_STATUS=0
run_case() {
    local expected_status=$1 downloads=$2 installs=$3 status=0
    : > "$TEST_LOG"
    : > "$TEST_WAITS"
    bash "$root/build-aux/windows/install-deps.sh" > "$work/output" 2>&1 || status=$?
    [[ $status == "$expected_status" ]]
    [[ $(grep -c '^-Sw ' "$TEST_LOG" || :) == "$downloads" ]]
    [[ $(grep -c '^-S ' "$TEST_LOG" || :) == "$installs" ]]
}
run_case 0 1 1
test ! -s "$TEST_WAITS"
TEST_DOWNLOAD_FAILURES=2
run_case 0 3 1
[[ $(cat "$TEST_WAITS") == $'10\n20' ]]
# Repeated download failures must never proceed to installation.
TEST_DOWNLOAD_FAILURES=3
run_case 7 3 0
# An installation failure is returned without retrying the transaction.
TEST_DOWNLOAD_FAILURES=0 TEST_INSTALL_STATUS=9
run_case 9 1 1
test ! -s "$TEST_WAITS"
MSYSTEM=MSYS
run_case 2 0 0
echo 'MSYS2 download retry and installation failure handling: passed'
