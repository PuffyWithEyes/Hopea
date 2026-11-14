#!/bin/sh
# SPDX-FileCopyrightText: 2019 Christopher Davis <brainblasted@disroot.org>
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
set -eu

# Work only in Meson's distribution tree. vendor/ contains patched path
# dependencies; never move or overwrite it in the developer's checkout.
cd "$2"
mkdir -p .cargo
cargo vendor --locked --versioned-dirs vendor-registry > .cargo/config.toml
