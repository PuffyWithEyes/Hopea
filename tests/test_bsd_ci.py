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
''')
                stub.chmod(0o755)
                for command in ("uname", "pkg", "pkg_add"):
                    (binary / command).symlink_to("stub")
                script = (ROOT / "packaging/bsd/deps.sh").read_text()
                # Redirect only host paths/tool discovery in the fixture copy.
                script = script.replace('export PATH=', '# export PATH=', 1)
                script = script.replace('/etc/pkg_install.conf', str(config))
                script = script.replace('/usr/pkg/etc/pkgin', str(pkgin_config))
                script = script.replace('command -v pkgin >/dev/null 2>&1', 'test -x "$TEST_BIN/pkgin"')
                path = root / "deps.sh"
                path.write_text(script)
                env = dict(os.environ, PATH=f"{binary}:{os.environ['PATH']}",
                           HOPEA_CI_CONTAINER="1", TEST_OS=system, TEST_LOG=str(log), TEST_BIN=str(binary))
                env.pop("PKG_PATH", None)
                result = subprocess.run(["sh", str(path)], env=env, capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stderr)
                calls = [json.loads(line) for line in log.read_text().splitlines()]
                if system == "NetBSD":
                    self.assertIn(["pkg_add", "pkgin"], calls)
                    self.assertLess(calls.index(["pkg_add", "pkgin"]), calls.index(["pkgin", "-y", "update"]))
                    self.assertEqual((pkgin_config / "repositories.conf").read_text().splitlines(),
                                     ["https://example.test/10.0_2026Q3/All", "https://mirror.test/All"])
                else:
                    install = next(call for call in calls if call[:2] == ["pkg", "install"])
                    self.assertIn("py312-pip-23.3.2_4", install)
                    self.assertIn("py312-sqlite3", install)
                    self.assertLess(calls.index(["pkg", "upgrade", "-y"]), calls.index(install))

    def test_ci_uses_vendored_tree_and_preserves_failure_status(self):
        for status in (0, 23):
            with self.subTest(status=status), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                scripts = root / "packaging/bsd"
                scripts.mkdir(parents=True)
                (scripts / "deps.sh").write_text("echo fixture-dependencies\n")
                source = root / ".packaging/bsd-source/hopea-1.2"
                (source / "vendor-registry").mkdir(parents=True)
                (source / ".cargo").mkdir()
                (source / ".cargo/config.toml").touch()
                build = source / "packaging/bsd/build.sh"
                build.parent.mkdir(parents=True)
                build.write_text('''set -eu
test -d vendor-registry
test "$TMPDIR" = "$PWD/.packaging/tmp"
test "$CARGO_HOME" = "$PWD/.packaging/cargo-home"
test "$XDG_CACHE_HOME" = "$PWD/.packaging/cache"
echo fixture-build
touch "$HOPEA_DIST_DIR/fixture.pkg"
exit "${TEST_STATUS}"
''')
                result = subprocess.run(["sh", str(ROOT / "packaging/bsd/ci.sh")], cwd=root,
                                        env=dict(os.environ, HOPEA_VERSION="1.2", TEST_STATUS=str(status)),
                                        capture_output=True, text=True)
                self.assertEqual(result.returncode, status, result.stdout + result.stderr)
                self.assertTrue((root / "dist/fixture.pkg").is_file())
                self.assertIn("fixture-build", (root / "dist/bsd-build.log").read_text())


if __name__ == "__main__":
    unittest.main()
