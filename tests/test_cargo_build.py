#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
"""Check Cargo output publication without requiring a Rust compilation."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

HELPER = Path(__file__).resolve().parents[1] / "build-aux/cargo-build.py"


class CargoBuildTest(unittest.TestCase):
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
            self.assertEqual(list(root.iterdir()), [output])


if __name__ == "__main__":
    unittest.main()
