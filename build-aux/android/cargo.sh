#!/bin/sh
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
set -eu
build_dir="$1"
case "$(basename "$build_dir")" in
  bin-aarch64) target=aarch64-linux-android ;;
  bin-armv7) target=armv7-linux-androideabi ;;
  bin-x86_64) target=x86_64-linux-android ;;
  *) echo 'Unsupported Android build directory/architecture' >&2; exit 1 ;;
esac
compiler=${target}28-clang
if [ "$target" = armv7-linux-androideabi ]; then
  compiler=armv7a-linux-androideabi28-clang
fi
tools=/opt/android/ndk/28.2.13676358/toolchains/llvm/prebuilt/linux-x86_64/bin
target_env=$(printf '%s' "$target" | tr '-' '_')
target_upper=$(printf '%s' "$target_env" | tr '[:lower:]' '[:upper:]')
export PKG_CONFIG_ALLOW_CROSS=1
export PKG_CONFIG_PATH="$build_dir/meson-uninstalled"
export "CARGO_TARGET_${target_upper}_LINKER=$tools/${compiler}"
export "CC_${target_env}=$tools/${compiler}"
export "AR_${target_env}=$tools/llvm-ar"
export CARGO_HOME=/work/.android-cargo
export HOPEA_ANDROID_RESOURCE="$build_dir/src/hopea.gresource"
cargo build --locked --lib --target "$target" --target-dir "$build_dir/cargo-target" --manifest-path /work/Cargo.toml
cp "$build_dir/cargo-target/$target/debug/libhopea.a" "$2"
