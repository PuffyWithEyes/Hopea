# SPDX-FileCopyrightText: 2022  Emmanuele Bassi
# SPDX-License-Identifier: GPL-3.0-or-later

# Discover once at configuration time. Reconfigure after adding/removing a file.
# The generated LINGUAS and copies are also used by msgfmt's desktop/XML merger,
# so every consumer uses the same catalog set without writing into the source.
translation_python = import('python').find_installation()
translation_codes = run_command(
  translation_python, files('../build-aux/list-translations.py'), meson.current_source_dir(),
  check: true,
).stdout().strip()
translation_linguas = configure_file(
  input: 'LINGUAS.in',
  output: 'LINGUAS',
  configuration: {'LANGUAGES': translation_codes},
)
translation_languages = translation_codes.split('\n')
translation_sources = [translation_linguas]
foreach language : translation_languages
  translation_sources += configure_file(input: language + '.po', output: language + '.po', copy: true)
endforeach
translation_po_dir = meson.current_build_dir()

i18n.gettext(
  'hopea',
  languages: translation_languages,
  args: [
    '--keyword=i18n',
    '--keyword=i18n_f',
    '--keyword=i18n_k',
    '--keyword=ni18n:1,2',
    '--keyword=ni18n_f:1,2',
    '--keyword=ni18n_k:1,2',
  ],
  preset: 'glib',
)
