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
# Bionic iconv on API 34 loops over the terminating NUL of UTF-8 messages,
# making GNU gettext grow its conversion buffer indefinitely. All shipped MO
# catalogs are UTF-8, so use gettext's native no-conversion configuration.
export am_cv_func_iconv=no
if [ -d "$build_dir/cargo-target" ] && [ ! -f "$build_dir/cargo-target/.gettext-utf8-only" ]; then
  # gettext-sys does not track Autoconf cache variables as Cargo inputs.
  cargo clean --package gettext-sys --target "$target" --target-dir "$build_dir/cargo-target" --manifest-path /work/Cargo.toml
fi
cargo build --locked --lib --target "$target" --target-dir "$build_dir/cargo-target" --manifest-path /work/Cargo.toml
touch "$build_dir/cargo-target/.gettext-utf8-only"
cp "$build_dir/cargo-target/$target/debug/libhopea.a" "$2"
