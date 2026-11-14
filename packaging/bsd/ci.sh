#!/bin/sh
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
set -eu
export PATH="/usr/local/sbin:/usr/local/bin:/usr/pkg/sbin:/usr/pkg/bin:/usr/pkg/gnu/bin:$PATH"
export HOPEA_CI_CONTAINER=1
mkdir -p dist
uname -a
if sh -x packaging/bsd/deps.sh >dist/bsd-dependencies.log 2>&1; then
  cat dist/bsd-dependencies.log
else
  status=$?
  cat dist/bsd-dependencies.log
  echo 'BSD dependency installation failed; see bsd-dependencies.log.' >&2
  exit "$status"
fi
exec bash -o pipefail -c 'bash -x packaging/bsd/build.sh 2>&1 | tee dist/bsd-build.log'
