#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
"""Install compiled Hopea catalogs into Android's extracted runtime assets."""
import gettext
import pathlib
import shutil
import sys


def install(source: pathlib.Path, destination: pathlib.Path) -> None:
    languages = (source / "LINGUAS").read_text().splitlines()
    languages = [line.strip() for line in languages if line.strip() and not line.startswith("#")]
    if not languages:
        raise ValueError("No translation catalogs were built")
    for language in languages:
        catalog = source / language / "LC_MESSAGES/hopea.mo"
        if not catalog.is_file():
            raise FileNotFoundError(catalog)
        with catalog.open("rb") as stream:
            if gettext.GNUTranslations(stream).charset().lower() != "utf-8":
                raise ValueError(f"Android requires UTF-8 translations: {catalog}")
        target = destination / language / "LC_MESSAGES/hopea.mo"
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(catalog, target)


if __name__ == "__main__":
    install(pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2]))
