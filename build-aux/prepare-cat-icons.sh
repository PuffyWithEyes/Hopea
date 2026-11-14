#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026  Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail
cd "$(dirname "$0")/.."
# Packaging sizes only; the illustration is generated, not redrawn here.
for size in 16 24 32 48 64 128 256 512; do
    magick data/icons/cat-kz/master.png -resize "${size}x${size}" \
        -strip "data/icons/cat-kz/${size}.png"
done
