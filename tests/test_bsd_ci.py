#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
"""Exercise BSD preparation with fake package tools, without changing the host."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class BsdCiTest(unittest.TestCase):
    def test_package_bootstrap_and_python_sqlite_pair(self):
        for system in ("FreeBSD", "DragonFly", "NetBSD"):
            with self.subTest(system=system), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                binary = root / "bin"
                binary.mkdir()
                log = root / "commands.jsonl"
                config = root / "pkg_install.conf"
                config.write_text("PKG_PATH=https://example.test/10.0_2026Q3/All;https://mirror.test/All\n")
                pkgin_config = root / "pkgin"
                tools = root / "packaging/bsd"
                tools.mkdir(parents=True)
                (tools / "netbsd-x11.sh").write_text('echo fixture-x11\n')
                stub = binary / "stub"
                stub.write_text(f"#!{sys.executable}\n" + '''
import json, os, pathlib, sys
name = pathlib.Path(sys.argv[0]).name
with open(os.environ["TEST_LOG"], "a") as log:
    log.write(json.dumps([name, *sys.argv[1:]]) + "\\n")
if name == "uname":
    print(os.environ["TEST_OS"])
elif name == "pkg" and sys.argv[1] == "search":
    print("py311-pip-23.3.2_4\\npy312-pip-23.3.2_4")
elif name == "pkg_add":
    pathlib.Path(sys.argv[0]).with_name("pkgin").symlink_to("stub")
elif name == "pkgin" and os.environ.get("TEST_PKG_FAILURE"):
    sys.exit(19)
elif name == "python3.13" and "venv" in sys.argv:
    destination = pathlib.Path(sys.argv[-1])
    (destination / "bin").mkdir(parents=True)
    compiler = destination / "bin/blueprint-compiler"
    compiler.write_text("#!/bin/sh\\nexit 0\\n")
    compiler.chmod(0o755)
''')
                stub.chmod(0o755)
                for command in ("uname", "pkg", "pkg_add", "python3.13"):
                    (binary / command).symlink_to("stub")
                script = (ROOT / "packaging/bsd/deps.sh").read_text()
                # Redirect only host paths/tool discovery in the fixture copy.
                script = script.replace('export PATH=', '# export PATH=', 1)
                script = script.replace('/etc/pkg_install.conf', str(config))
                script = script.replace('/usr/pkg/etc/pkgin', str(pkgin_config))
                script = script.replace('/usr/local/lib/hopea-blueprint', str(root / 'blueprint'))
                script = script.replace('/usr/local/bin', str(binary))
                package_errors = root / "pkg_install-err.log"
                package_errors.write_text("fixture package installation error\n")
                script = script.replace('/var/db/pkgin/pkg_install-err.log', str(package_errors))
                script = script.replace('command -v pkgin >/dev/null 2>&1', 'test -x "$TEST_BIN/pkgin"')
                path = root / "deps.sh"
                path.write_text(script)
                env = dict(os.environ, PATH=f"{binary}:{os.environ['PATH']}",
                           HOPEA_CI_CONTAINER="1", TEST_OS=system, TEST_LOG=str(log), TEST_BIN=str(binary))
                env.pop("PKG_PATH", None)
                result = subprocess.run(["sh", str(path)], cwd=root, env=env, capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stderr)
                calls = [json.loads(line) for line in log.read_text().splitlines()]
                if system == "NetBSD":
                    self.assertIn("fixture-x11", result.stdout)
                    self.assertIn(["pkg_add", "pkgin"], calls)
                    self.assertLess(calls.index(["pkg_add", "pkgin"]), calls.index(["pkgin", "-y", "update"]))
                    self.assertEqual((pkgin_config / "repositories.conf").read_text().splitlines(),
                                     ["https://example.test/10.0_2026Q3/All", "https://mirror.test/All"])
                    failed = subprocess.run(["sh", str(path)], cwd=root,
                                            env=dict(env, TEST_PKG_FAILURE="1"),
                                            capture_output=True, text=True)
                    self.assertEqual(failed.returncode, 19)
                    self.assertIn("fixture package installation error", failed.stderr)
                else:
                    install = next(call for call in calls if call[:2] == ["pkg", "install"])
                    self.assertIn("py312-pip-23.3.2_4", install)
                    self.assertIn("py312-sqlite3", install)
                    self.assertLess(calls.index(["pkg", "upgrade", "-y"]), calls.index(install))

    def test_ci_uses_vendored_tree_and_preserves_failure_status(self):
        for status in (0, 23):
            with self.subTest(status=status), tempfile.TemporaryDirectory() as directory:
                root = Path(directory) / "checkout"
                root.mkdir()
                scripts = root / "packaging/bsd"
                scripts.mkdir(parents=True)
                (scripts / "deps.sh").write_text("echo fixture-dependencies\n")
                source = root / ".packaging/bsd-source/hopea-1.2"
                (source / "vendor-registry").mkdir(parents=True)
                (source / ".cargo").mkdir()
                (root / "Cargo.toml").write_text('[workspace]\nmembers = []\n')
                (source / "Cargo.toml").write_text('[workspace]\nmembers = []\n')
                # Foreign Rust builds must not inherit the vendored source's
                # compiler/configuration, even if their own dependencies differ.
                (source / ".cargo/config.toml").write_text('[build]\nrustc = "missing-hopea-only-compiler"\n')
                probe = root / "probe.py"
                probe.write_text('''import json, os, pathlib, subprocess, tempfile
with tempfile.TemporaryDirectory() as directory:
    crate = pathlib.Path(directory) / "maturin-fixture"
    (crate / "src").mkdir(parents=True)
    (crate / "Cargo.toml").write_text('[package]\\nname="foreign-build"\\nversion="0.1.0"\\nedition="2021"\\n')
    (crate / "src/lib.rs").write_text('pub fn test() {}\\n')
    command = ["cargo", "metadata", "--offline", "--no-deps", "--format-version", "1"]
    metadata = json.loads(subprocess.check_output(command, cwd=crate))
    assert pathlib.Path(metadata["workspace_root"]).resolve() == crate.resolve()
    subprocess.run(["cargo", "check", "--offline"], cwd=crate, check=True)
''')
                build = source / "packaging/bsd/build.sh"
                build.parent.mkdir(parents=True)
                build.write_text('''set -eu
test -d vendor-registry
test "$PIP_NO_CACHE_DIR" = 1
"$TEST_PYTHON" "$TEST_PROBE"
echo fixture-build
touch "$HOPEA_DIST_DIR/fixture.pkg"
exit "${TEST_STATUS}"
''')
                result = subprocess.run(["sh", str(ROOT / "packaging/bsd/ci.sh")], cwd=root,
                                        env=dict(os.environ, HOPEA_VERSION="1.2", TEST_STATUS=str(status),
                                                 TEST_PYTHON=sys.executable, TEST_PROBE=str(probe)),
                                        capture_output=True, text=True)
                self.assertEqual(result.returncode, status, result.stdout + result.stderr)
                self.assertTrue((root / "dist/fixture.pkg").is_file())
                self.assertIn("fixture-build", (root / "dist/bsd-build.log").read_text())
                self.assertEqual(list(Path(directory).glob("hopea-bsd-tools.*")), [])


if __name__ == "__main__":
    unittest.main()
