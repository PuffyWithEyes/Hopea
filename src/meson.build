# SPDX-FileCopyrightText: 2022  Emmanuele Bassi
# SPDX-License-Identifier: GPL-3.0-or-later

subdir('gtk')

pkgdatadir = get_option('prefix') / get_option('datadir') / meson.project_name()

gnome.compile_resources('hopea',
  'hopea.gresource.xml',
  gresource_bundle: true,
  build_by_default: true,
  dependencies: blueprints,
  install: true,
  install_dir: pkgdatadir,
)

conf = configuration_data()
conf.set_quoted('VERSION', '@0@@1@'.format(meson.project_version(), version_suffix))
conf.set_quoted('GETTEXT_PACKAGE', 'hopea')
conf.set_quoted('LOCALEDIR', get_option('prefix') / get_option('localedir'))
conf.set_quoted('DEVEL_LOCALEDIR', meson.project_build_root() / 'po')
conf.set_quoted('PKGDATADIR', pkgdatadir)
conf.set_quoted('APPLICATION_ID', application_id)
conf.set_quoted('APPLICATION_ICON', application_id + '.Cat')
conf.set_quoted('PROFILE', get_option('profile'))

config_rs = configure_file(
  input: 'config.rs.in',
  output: 'config.rs',
  configuration: conf
)

# Copy the config.rs output to the source directory.
run_command(
  'cp',
  meson.project_build_root() / 'src' / 'config.rs',
  meson.project_source_root() / 'src' / 'config.rs',
  check: true,
)

build_python = import('python').find_installation()
cargo_builder = files('../build-aux/cargo-build.py')
cargo_cache = get_option('cargo-cache-dir')
cargo_target_dir = meson.project_build_root() / 'src'
if cargo_cache != ''
  cargo_target_dir = cargo_cache / 'target'
endif
cargo_options = [ '--manifest-path', meson.project_source_root() / 'Cargo.toml' ]
cargo_options += [ '--target-dir', cargo_target_dir, '--locked' ]

if get_option('profile') == 'default'
  cargo_options += [ '--release' ]
  rust_target = 'release'
  message('Building in release mode')
else
  rust_target = 'debug'
  message('Building in debug mode')
endif

# Development builds use the crates already downloaded for other Rust builds;
# release builds (such as Flatpak's) keep their own copy in the build directory
cargo_env = environment()
if cargo_cache != ''
  cargo_env.set('CARGO_HOME', cargo_cache / 'home')
elif get_option('profile') == 'default'
  cargo_env.set('CARGO_HOME', meson.project_build_root() / 'cargo-home')
endif

player_binary = meson.project_name()
if host_machine.system() == 'windows'
  player_binary += '.exe'
endif
cargo_release = custom_target(
  'cargo-build',
  build_by_default: true,
  build_always_stale: true,
  output: player_binary,
  console: true,
  install: true,
  install_dir: get_option('bindir'),
  env: cargo_env,
  command: [
    build_python, cargo_builder,
    cargo_target_dir / rust_target / player_binary, '@OUTPUT@',
    cargo, 'build',
    cargo_options,
  ],
)

# Separate Cargo invocation keeps librespot's features out of the P2P node.
# Both binaries share Cargo.lock and are installed next to each other.
spotify_binary = 'hopea-spotify'
if host_machine.system() == 'windows'
  spotify_binary += '.exe'
endif
custom_target(
  'spotify-worker',
  build_by_default: true,
  build_always_stale: true,
  output: spotify_binary,
  console: true,
  install: true,
  install_dir: get_option('bindir'),
  env: cargo_env,
  command: [
    build_python, cargo_builder,
    cargo_target_dir / rust_target / spotify_binary, '@OUTPUT@',
    cargo, 'build', cargo_options, '--package', 'hopea-spotify',
  ],
)
