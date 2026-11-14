#!/bin/sh
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
set -eu
build_dir="$1"
export PKG_CONFIG_ALLOW_CROSS=1
export PKG_CONFIG_PATH="$build_dir/meson-uninstalled"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER=/opt/android/ndk/28.2.13676358/toolchains/llvm/prebuilt/linux-x86_64/bin/aarch64-linux-android31-clang
export CC_aarch64_linux_android="$CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER"
export AR_aarch64_linux_android=/opt/android/ndk/28.2.13676358/toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-ar
export CARGO_HOME=/work/.android-cargo
export HOPEA_ANDROID_RESOURCE="$build_dir/src/hopea.gresource"
cargo build --lib --target aarch64-linux-android --target-dir "$build_dir/cargo-target" --manifest-path /work/Cargo.toml
cp "$build_dir/cargo-target/aarch64-linux-android/debug/libhopea.a" "$2"
