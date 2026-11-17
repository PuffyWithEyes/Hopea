# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
"""Adapt the pinned Pixiewood builder, including already cached toolchains."""
from pathlib import Path

builder = Path('/opt/pixiewood')
path = builder / 'pixiewood'
text = path.read_text()
old = 'Set::Scalar->new("aarch64", "x86_64")'
new = 'Set::Scalar->new("aarch64", "x86_64", "armv7")'
if old not in text and new not in text:
    raise SystemExit('Unexpected Pixiewood architecture selection')
path.write_text(text.replace(old, new))

# Rust calls this target armv7; the NDK compiler uses armv7a instead.
(builder / 'prepare/arch/armv7.cross').write_text("""[constants]
arch = 'armv7a'
abi = 'armeabi-v7a'
platform = 'androideabi'
api = '28'
rust_target = 'armv7-linux-androideabi'

[host_machine]
cpu_family = 'arm'
cpu = 'armv7'
endian = 'little'
""")
for arch in ['aarch64', 'x86_64']:
    path = builder / f'prepare/arch/{arch}.cross'
    text = path.read_text()
    if 'rust_target =' not in text:
        text = text.replace('[constants]', f"[constants]\napi = '28'\nrust_target = '{arch}-linux-android'")
    path.write_text(text)

for path, old, new in [
    (Path('/opt/pixiewood/prepare/android.cross'), "platform+'31-clang", "platform+api+'-clang"),
    (Path('/opt/pixiewood/generate/gradle.xsl'), 'minSdk 31', 'minSdk 28'),
]:
    text = path.read_text()
    # Accept the previous API 28 adapter in cached images too.
    text = text.replace("platform+'28-clang", "platform+api+'-clang")
    if old not in text and new not in text:
        raise SystemExit(f'Unexpected Android toolchain configuration: {path}')
    path.write_text(text.replace(old, new))
