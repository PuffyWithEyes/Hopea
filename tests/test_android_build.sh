#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/bin"
# Check that Cargo, C dependencies and the copied archive all use the same
# architecture. A stub avoids rebuilding GTK or accessing the user's Cargo home.
cat > "$work/bin/cargo" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
[[ $* == *'--locked'* ]]
while (($#)); do
  case "$1" in
    --target) target=$2; shift ;;
    --target-dir) directory=$2; shift ;;
  esac
  shift
done
lower=${target//-/_}
upper=${lower^^}
linker=CARGO_TARGET_${upper}_LINKER
compiler=CC_${lower}
archiver=AR_${lower}
[[ ${!linker} == */"${target}31-clang" ]]
[[ ${!compiler} == "${!linker}" && ${!archiver} == */llvm-ar ]]
[[ $PKG_CONFIG_PATH == "${directory%/cargo-target}/meson-uninstalled" ]]
mkdir -p "$directory/$target/debug"
printf '%s\n' "$target" > "$directory/$target/debug/libhopea.a"
EOF
chmod +x "$work/bin/cargo"
for arch in aarch64 x86_64; do
  build_dir="$work/bin-$arch"
  mkdir -p "$build_dir"
  PATH="$work/bin:$PATH" sh "$root/build-aux/android/cargo.sh" "$build_dir" "$work/$arch.a"
  [[ $(cat "$work/$arch.a") == "$arch-linux-android" ]]
done
if PATH="$work/bin:$PATH" sh "$root/build-aux/android/cargo.sh" "$work/bin-unknown" "$work/bad.a" 2>/dev/null; then
  echo 'Unsupported architecture accepted' >&2; exit 1
fi
test ! -e "$work/bad.a"
echo 'Android Cargo/C architecture selection: passed'
