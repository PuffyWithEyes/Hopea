#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
"""Discover gettext catalogs; po/<locale>.po is the sole language registry."""

from pathlib import Path
import re
import sys


def languages(directory):
    result = []
    for path in sorted(Path(directory).glob("*.po")):
        code = path.stem
        if not re.fullmatch(r"[a-z]{2,3}(?:_[A-Za-z0-9]{2,8}){0,2}(?:@[a-z0-9-]{1,16})?", code):
            raise ValueError(f"Invalid translation filename: {path.name}; use <locale>.po (e.g. ru.po or pt_BR.po)")
        if not path.is_file():
            raise ValueError(f"Not a translation file: {path}")
        result.append(code)
    if not result:
        raise ValueError("No .po catalogs found")
    return result


if __name__ == "__main__":
    try:
        print("\n".join(languages(sys.argv[1])))
    except ValueError as error:
        sys.exit(str(error))
