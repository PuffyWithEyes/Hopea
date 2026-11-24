# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
"""Reject stale Gradle outputs, even when dependency resolution succeeds."""
import importlib.util
from pathlib import Path
import tempfile
import unittest
import zipfile

source = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("runtime_check", source / "build-aux/android/check-import-runtime.py")
runtime_check = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runtime_check)


def archive(path, entries):
    path.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(path, "w") as output:
        for name, data in entries.items():
            output.writestr(name, data)


class RuntimeCheckTest(unittest.TestCase):
    def test_rejects_stale_native_and_standard_library_components(self):
        for stale in (None, "libpython", "ssl-native", "ssl.pyc", "http/cookiejar.pyc", "tarfile.pyc"):
            with self.subTest(stale=stale), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                target = root / "maven/com/chaquo/python/target" / runtime_check.RELEASE
                prefix = f"target-{runtime_check.RELEASE}"
                archive(target / f"{prefix}-arm64-v8a.zip", {
                    "jniLibs/arm64-v8a/libpython3.11.so": b"new-native",
                    "lib-dynload/arm64-v8a/_ssl.cpython-311.so": b"new-ssl",
                })
                modules = {name: b"new-module" for name in ("ssl.pyc", "http/cookiejar.pyc", "tarfile.pyc")}
                archive(target / f"{prefix}-stdlib-pyc.zip", modules)
                libraries = root / "libraries"
                (libraries / "arm64-v8a").mkdir(parents=True)
                (libraries / "arm64-v8a/libpython3.11.so").write_bytes(b"old" if stale == "libpython" else b"new-native")
                assets = root / "assets"
                archive(assets / "stdlib-arm64-v8a.imy", {
                    "_ssl.cpython-311.so": b"old" if stale == "ssl-native" else b"new-ssl",
                })
                if stale in modules:
                    modules[stale] = b"old"
                archive(assets / "stdlib-common.imy", modules)
                if stale:
                    with self.assertRaises(ValueError):
                        runtime_check.check(root, libraries, assets, "arm64-v8a")
                else:
                    runtime_check.check(root, libraries, assets, "arm64-v8a")


if __name__ == "__main__":
    unittest.main()
