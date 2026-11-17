# SPDX-FileCopyrightText: 2025  Emmanuele Bassi
# SPDX-License-Identifier: GPL-3.0-or-later

blueprints = custom_target('blueprints',
  input: files(
    'library-panel.blp',
    'playback-control.blp',
    'playlist-view.blp',
    'queue-row.blp',
    'shortcuts-dialog.blp',
    'song-cover.blp',
    'song-details.blp',
    'volume-control.blp',
    'window.blp',
  ),
  output: [
    'library-panel.ui',
    'playback-control.ui',
    'playlist-view.ui',
    'queue-row.ui',
    'shortcuts-dialog.ui',
    'song-cover.ui',
    'song-details.ui',
    'volume-control.ui',
    'window.ui',
  ],
  command: [
    find_program('blueprint-compiler'),
    'batch-compile',
    '@OUTDIR@',
    '@CURRENT_SOURCE_DIR@',
    '@INPUT@',
  ],
)
