#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
"""Check Cargo output publication without requiring a Rust compilation."""
import json
import os
from pathlib import Path
import subprocess
import shutil
import sys
import tempfile
import unittest

HELPER = Path(__file__).resolve().parents[1] / "build-aux/cargo-build.py"


class CargoBuildTest(unittest.TestCase):
    @unittest.skipUnless(shutil.which("cargo"), "Cargo is required for the offline build fixture")
    def test_sibling_build_directory_finds_vendored_dependencies(self):
        with tempfile.TemporaryDirectory(prefix="hopea offline ") as directory:
            root = Path(directory)
            source, build = root / "source", root / "build"
            (source / "src").mkdir(parents=True)
            (source / ".cargo").mkdir()
            build.mkdir()
            dependency = source / "vendor-registry/hex-0.4.3"
            (dependency / "src").mkdir(parents=True)
            (dependency / "Cargo.toml").write_text('[package]\nname="hex"\nversion="0.4.3"\nedition="2021"\n')
            (dependency / "src/lib.rs").write_text('pub fn fixture() -> u8 { 42 }\n')
            (dependency / ".cargo-checksum.json").write_text('{"files":{},"package":null}')
            (source / ".cargo/config.toml").write_text(
                '[source.crates-io]\nreplace-with="fixture"\n'
                '[source.fixture]\ndirectory="vendor-registry"\n')
            (source / "Cargo.toml").write_text(
                '[package]\nname="offline-fixture"\nversion="0.0.0"\nedition="2021"\n'
                '[dependencies]\nhex="=0.4.3"\n[workspace]\n')
            (source / "src/main.rs").write_text('fn main() { assert_eq!(hex::fixture(), 42); }\n')
            executable = "offline-fixture.exe" if os.name == "nt" else "offline-fixture"
            result = subprocess.run([
                sys.executable, str(HELPER), str(build / "target/debug" / executable), "published.exe",
                shutil.which("cargo"), "build", "--offline", "--manifest-path", "../source/Cargo.toml",
                "--target-dir", str(build / "target"),
            ], cwd=build, env=dict(os.environ, CARGO_HOME=str(root / "empty-cargo-home")),
                capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertTrue((build / "published.exe").is_file())

    def test_success_preserves_arguments_and_replaces_binary(self):
        with tempfile.TemporaryDirectory(prefix="hopea build ") as directory:
            root = Path(directory)
            source = root / "cargo cache & binary.exe"
            output = root / "installed binary.exe"
            output.write_text("previous binary")
            command = [sys.executable, "-c", (
                "import json, pathlib, sys; "
                "p = pathlib.Path(sys.argv[1]); "
                "p.write_text(json.dumps(sys.argv[2:])); p.chmod(0o755)"
            ), str(source), "--package", "hopea-spotify", "literal && argument"]
            result = subprocess.run([sys.executable, str(HELPER), str(source), str(output), *command])
            self.assertEqual(result.returncode, 0)
            self.assertEqual(json.loads(output.read_text()), command[-3:])
            self.assertEqual(source.read_bytes(), output.read_bytes())
            if os.name != "nt":
                self.assertTrue(os.access(output, os.X_OK))
            self.assertEqual(sorted(p.name for p in root.iterdir()), sorted([source.name, output.name]))

    def test_failed_build_preserves_installed_binary(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source, output = root / "stale.exe", root / "output.exe"
            source.write_text("stale cargo output")
            output.write_text("working binary")
            result = subprocess.run([
                sys.executable, str(HELPER), str(source), str(output),
                sys.executable, "-c", "import sys; sys.exit(23)",
            ])
            self.assertEqual(result.returncode, 23)
            self.assertEqual(output.read_text(), "working binary")
            self.assertEqual(len(list(root.iterdir())), 2)

    def test_missing_output_preserves_binary_and_cleans_temporary_file(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            output = root / "output.exe"
            output.write_text("working binary")
            result = subprocess.run([
                sys.executable, str(HELPER), str(root / "missing.exe"), str(output),
                sys.executable, "-c", "pass",
            ], capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(output.read_text(), "working binary")
            self.assertEqual([p.name for p in root.iterdir()], [output.name])


if __name__ == "__main__":
    unittest.main()
