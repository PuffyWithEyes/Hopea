# SPDX-FileCopyrightText: 2022  Emmanuele Bassi
# SPDX-License-Identifier: GPL-3.0-or-later

foreach size : [16, 24, 32, 48, 64, 128, 256, 512]
  install_data(
    'cat-kz' / '@0@.png'.format(size),
    rename: application_id + '.Cat.png',
    install_dir: get_option('datadir') / 'icons/hicolor' / '@0@x@0@/apps'.format(size),
  )
endforeach
