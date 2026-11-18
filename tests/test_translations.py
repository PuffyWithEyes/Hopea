#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
"""Exercise drop-in PO catalogs with real Meson/gettext and temporary sources."""

import gettext
import importlib.util
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("translations", ROOT / "build-aux/list-translations.py")
translations = importlib.util.module_from_spec(spec)
spec.loader.exec_module(translations)


def catalog(language, translation):
    return f'''msgid ""
msgstr ""
"Language: {language}\\n"
"Content-Type: text/plain; charset=UTF-8\\n"
"Plural-Forms: nplurals=2; plural=(n != 1);\\n"

msgid "Music"
msgstr "{translation}"
'''


class TranslationTest(unittest.TestCase):
    def test_names_and_flat_directory(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for name in ("pt_BR", "az", "sr_Latn_RS", "sr@latin", "es_419"):
                (root / f"{name}.po").touch()
            (root / "review").mkdir()
            (root / "review/unfinished.po").touch()
            self.assertEqual(translations.languages(root), ["az", "es_419", "pt_BR", "sr@latin", "sr_Latn_RS"])
            (root / "ru-copy.po").touch()
            with self.assertRaisesRegex(ValueError, "Invalid translation filename"):
                translations.languages(root)

    def test_new_catalog_builds_installs_and_updates_without_registry(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source, build, install = (root / name for name in ("source", "build", "install"))
            (source / "po").mkdir(parents=True)
            (source / "build-aux").mkdir()
            for file in ("po/meson.build", "po/LINGUAS.in", "build-aux/list-translations.py"):
                shutil.copyfile(ROOT / file, source / file)
            (source / "meson.build").write_text('''project('hopea-translations-test')
i18n = import('i18n')
fs = import('fs')
subdir('po')
custom_target('desktop', input: 'test.desktop.in', output: 'test.desktop',
  command: [find_program('msgfmt'), '--desktop', '-d', translation_po_dir, '--template', '@INPUT@', '-o', '@OUTPUT@'],
  depend_files: translation_sources, build_by_default: true)
''')
            (source / "test.desktop.in").write_text('[Desktop Entry]\nType=Application\nName=Music\nExec=true\n')
            (source / "po/ru.po").write_text(catalog("ru", "Музыка"))
            def run(*args):
                result = subprocess.run(args, cwd=source, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=90)
                self.assertEqual(result.returncode, 0, result.stdout)
            run("meson", "setup", str(build), str(source))
            run("meson", "compile", "-C", str(build))
            self.assertIn("Name[ru]=Музыка", (build / "test.desktop").read_text())
            # A new locale needs only its PO and reconfiguration, including for
            # desktop integration. There is no source LINGUAS to maintain.
            (source / "po/az.po").write_text(catalog("az", "Musiqi"))
            run("meson", "setup", "--reconfigure", str(build), str(source))
            run("meson", "compile", "-C", str(build))
            run("meson", "install", "-C", str(build), "--destdir", str(install))
            self.assertIn("Name[az]=Musiqi", (build / "test.desktop").read_text())
            with (install / "usr/local/share/locale/az/LC_MESSAGES/hopea.mo").open("rb") as stream:
                self.assertEqual(gettext.GNUTranslations(stream).gettext("Music"), "Musiqi")
            self.assertFalse((source / "po/LINGUAS").exists())
            # Editing an existing PO automatically refreshes both consumers.
            (source / "po/az.po").write_text(catalog("az", "Updated"))
            run("meson", "compile", "-C", str(build))
            self.assertIn("Name[az]=Updated", (build / "test.desktop").read_text())
            with (build / "po/az/LC_MESSAGES/hopea.mo").open("rb") as stream:
                self.assertEqual(gettext.GNUTranslations(stream).gettext("Music"), "Updated")
            # Android's asset collector does not follow these independent
            # Meson targets: explicitly package the identical catalog set.
            assets = root / "android-assets/share/locale"
            packager = ROOT / "build-aux/android/package-translations.py"
            run(sys.executable, str(packager), str(build / "po"), str(assets))
            with (assets / "az/LC_MESSAGES/hopea.mo").open("rb") as stream:
                self.assertEqual(gettext.GNUTranslations(stream).gettext("Music"), "Updated")
            self.assertTrue((assets / "ru/LC_MESSAGES/hopea.mo").is_file())


if __name__ == "__main__":
    unittest.main()
