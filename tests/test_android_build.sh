#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
# Apply the real Android port to current sources before testing its Cargo shim.
# This catches desktop build changes that leave the staging patch out of date.
mkdir -p "$work/source"
while IFS= read -r source; do
  mkdir -p "$work/source/$(dirname "$source")"
  cp "$root/$source" "$work/source/$source"
done < <(sed -n 's|^--- a/||p' "$root/build-aux/android/port.patch")
case ${1:-} in
  --staged)
    # The container receives an already-patched tree. Validate that state in
    # the temporary copy before testing forward application again.
    patch --reverse --batch --fuzz=0 -d "$work/source" -p1 < "$root/build-aux/android/port.patch"
    ;;
  '') ;;
  *) echo 'Usage: test_android_build.sh [--staged]' >&2; exit 2 ;;
esac
patch --batch --fuzz=0 -d "$work/source" -p1 < "$root/build-aux/android/port.patch"
grep -Fq "rust_archive = custom_target('hopea-rust'" "$work/source/src/meson.build"
if grep -Fq 'cargo-build.py' "$work/source/src/meson.build"; then
  echo 'Desktop Cargo helper leaked into Android build' >&2; exit 1
fi
if [[ ${1:-} != --staged ]]; then
  # Also exercise the entry point used inside the Android container.
  mkdir -p "$work/source/tests" "$work/source/build-aux/android"
  cp "$root/tests/test_android_build.sh" "$work/source/tests/"
  cp "$root/build-aux/android/port.patch" "$root/build-aux/android/cargo.sh" "$work/source/build-aux/android/"
  bash "$work/source/tests/test_android_build.sh" --staged
fi
mkdir -p "$work/bin"
# Check that Cargo, C dependencies and the copied archive all use the same
# architecture. A stub avoids rebuilding GTK or accessing the user's Cargo home.
cat > "$work/bin/cargo" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
[[ $am_cv_func_iconv == no ]]
operation=$1
if [[ $operation == build ]]; then [[ $* == *'--locked'* ]]; fi
while (($#)); do
  case "$1" in
    --target) target=$2; shift ;;
    --target-dir) directory=$2; shift ;;
  esac
  shift
done
if [[ $operation == clean ]]; then
  [[ $* == '' ]]
  touch "$directory/cleaned-gettext"
  exit 0
fi
lower=${target//-/_}
upper=${lower^^}
linker=CARGO_TARGET_${upper}_LINKER
compiler=CC_${lower}
archiver=AR_${lower}
expected=${target}28-clang
if [[ $target == armv7-linux-androideabi ]]; then expected=armv7a-linux-androideabi28-clang; fi
[[ ${!linker} == */"$expected" ]]
[[ ${!compiler} == "${!linker}" && ${!archiver} == */llvm-ar ]]
[[ $PKG_CONFIG_PATH == "${directory%/cargo-target}/meson-uninstalled" ]]
mkdir -p "$directory/$target/debug"
printf '%s\n' "$target" > "$directory/$target/debug/libhopea.a"
EOF
chmod +x "$work/bin/cargo"
for arch in aarch64 x86_64 armv7; do
  build_dir="$work/bin-$arch"
  mkdir -p "$build_dir/cargo-target"
  PATH="$work/bin:$PATH" sh "$root/build-aux/android/cargo.sh" "$build_dir" "$work/$arch.a"
  expected=$arch-linux-android
  if [[ $arch == armv7 ]]; then expected=armv7-linux-androideabi; fi
  [[ $(cat "$work/$arch.a") == "$expected" ]]
  test -f "$build_dir/cargo-target/cleaned-gettext"
  test -f "$build_dir/cargo-target/.gettext-utf8-only"
  rm "$build_dir/cargo-target/cleaned-gettext"
  PATH="$work/bin:$PATH" sh "$root/build-aux/android/cargo.sh" "$build_dir" "$work/$arch.a"
  test ! -e "$build_dir/cargo-target/cleaned-gettext"
done
if PATH="$work/bin:$PATH" sh "$root/build-aux/android/cargo.sh" "$work/bin-unknown" "$work/bad.a" 2>/dev/null; then
  echo 'Unsupported architecture accepted' >&2; exit 1
fi
test ! -e "$work/bad.a"
echo 'Android Cargo/C architecture selection: passed'
