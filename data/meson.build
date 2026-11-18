# SPDX-FileCopyrightText: 2022  Emmanuele Bassi
# SPDX-License-Identifier: GPL-3.0-or-later

desktop_data = configuration_data()
desktop_data.set('APPLICATION_ID', application_id)
translation_msgfmt = find_program('msgfmt')
desktop_file = custom_target('desktop-translations',
  input: configure_file(
    input: 'io.hopea.Hopea.desktop.in.in',
    output: 'io.hopea.Hopea.desktop.in',
    configuration: desktop_data,
  ),
  output: '@0@.desktop'.format(application_id),
  command: [translation_msgfmt, '--desktop', '-d', translation_po_dir, '--template', '@INPUT@', '-o', '@OUTPUT@'],
  depend_files: translation_sources,
  build_by_default: true,
  install: true,
  install_dir: get_option('datadir') / 'applications',
)

desktop_utils = find_program('desktop-file-validate', required: false)
if desktop_utils.found()
  test('Validate desktop file',
    desktop_utils,
    args: [desktop_file],
    suite: ['lint'],
  )
endif

appstream_data = configuration_data()
appstream_data.set('APPLICATION_ID', application_id)
appstream_file = custom_target('appstream-translations',
  input: configure_file(
    input: 'io.hopea.Hopea.metainfo.xml.in.in',
    output: 'io.hopea.Hopea.metainfo.xml.in',
    configuration: appstream_data,
  ),
  output: '@0@.metainfo.xml'.format(application_id),
  command: [translation_msgfmt, '--xml', '-d', translation_po_dir, '--template', '@INPUT@', '-o', '@OUTPUT@'],
  depend_files: translation_sources,
  build_by_default: true,
  install: true,
  install_dir: get_option('datadir') / 'metainfo',
)

appstreamcli = find_program('appstreamcli', required: false)
if appstreamcli.found()
 test('Validate appstream file',
   appstreamcli,
   args: ['validate', '--no-net', '--explain', appstream_file],
   suite: ['lint'],
 )
endif

install_data('io.hopea.Hopea.gschema.xml',
  install_dir: get_option('datadir') / 'glib-2.0/schemas',
)

# Compile schemas locally, so we can run uninstalled under a devenv
gnome.compile_schemas(depend_files: files('io.hopea.Hopea.gschema.xml'))

compile_schemas = find_program('glib-compile-schemas', required: false)
if compile_schemas.found()
  test('Validate schema file',
    compile_schemas,
    args: ['--strict', '--dry-run', meson.current_source_dir()],
    suite: ['lint'],
  )
endif

service_conf = configuration_data()
service_conf.set('application_id', application_id)
service_conf.set('bindir', get_option('prefix') / get_option('bindir'))
configure_file(
  input: 'io.hopea.Hopea.service.in',
  output: '@0@.service'.format(application_id),
  configuration: service_conf,
  install: true,
  install_dir: get_option('datadir') / 'dbus-1/services',
)

subdir('icons')
