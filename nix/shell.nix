# SPDX-FileCopyrightText: 2026  Daniil Klimov
# SPDX-License-Identifier: CC0-1.0

# Development shell with everything needed to build the client and the
# peer-to-peer node: nix-shell nix/shell.nix --run 'meson setup _builddir . -Dprofile=development'
{
  pkgs ? import <nixpkgs> { },
}:

pkgs.mkShell {
  nativeBuildInputs = with pkgs; [
    appstream
    blueprint-compiler
    cargo
    desktop-file-utils
    ffmpeg
    gettext
    libxml2
    meson
    ninja
    pkg-config
    python3
    reuse
    rustc
    rustfmt
    typos
  ];

  buildInputs = with pkgs; [
    dbus
    glib
    glib-networking
    gtk4
    libadwaita
    libsoup_3
    openssl
    gst_all_1.gstreamer
    gst_all_1.gst-plugins-base
    gst_all_1.gst-plugins-good
    gst_all_1.gst-plugins-bad
  ];

  # GTK's own settings schemas (its file chooser aborts without them, when
  # no portal provides one) and icons, as wrapGAppsHook would set them up
  # for an installed app
  shellHook = ''
    export GIO_EXTRA_MODULES="${pkgs.glib-networking}/lib/gio/modules''${GIO_EXTRA_MODULES:+:$GIO_EXTRA_MODULES}"
    if [ -x "$PWD/.venv-import/bin/python" ]; then
      export HOPEA_IMPORT_PYTHON="$PWD/.venv-import/bin/python"
    fi
    export XDG_DATA_DIRS="${pkgs.gtk4}/share/gsettings-schemas/${pkgs.gtk4.name}:${pkgs.gsettings-desktop-schemas}/share/gsettings-schemas/${pkgs.gsettings-desktop-schemas.name}:${pkgs.adwaita-icon-theme}/share:${pkgs.hicolor-icon-theme}/share:$XDG_DATA_DIRS"
  '';
}
