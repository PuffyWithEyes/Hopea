#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
"""Refresh GTK's 128-byte resource fingerprint after all packaging additions."""
import hashlib
import sys
from pathlib import Path

root = Path(sys.argv[1])
fingerprint = hashlib.shake_256()
for path in sorted(root.rglob('*')):
    if not path.is_file() or path == root / 'afpr':
        continue
    relative = path.relative_to(root).as_posix().encode('utf-8')
    fingerprint.update(len(relative).to_bytes(8, 'big'))
    fingerprint.update(relative)
    fingerprint.update(path.stat().st_size.to_bytes(8, 'big'))
    with path.open('rb') as stream:
        while chunk := stream.read(1024 * 1024):
            fingerprint.update(chunk)
(root / 'afpr').write_bytes(fingerprint.digest(128))
