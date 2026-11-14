# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: CC0-1.0
{
  lib,
  stdenv,
  rustPlatform,
  blueprint-compiler,
  cargo,
  desktop-file-utils,
  meson,
  ninja,
  pkg-config,
  rustc,
  m4,
  gettext,
  wrapGAppsHook4,
  glib,
  glib-networking,
  gtk4,
  gst_all_1,
  libadwaita,
  libsoup_3,
  openssl,
  dbus,
  python3,
  ffmpeg,
  adwaita-icon-theme,
  hicolor-icon-theme,
  gsettings-desktop-schemas,
}:
let
  python = python3.override {
    packageOverrides = final: prev: {
      vk-api = final.buildPythonPackage {
        pname = "vk-api";
        version = "11.10.1";
        pyproject = true;
        src = final.fetchPypi {
          pname = "vk_api";
          version = "11.10.1";
          hash = "sha256-iOr1q3/hcfW2fel221PJ4b7e/GswAX42SlRLIt0Df94=";
        };
        build-system = [ final.setuptools ];
        dependencies = [
          final.requests
          final.beautifulsoup4
        ];
        pythonImportsCheck = [
          "vk_api"
          "vk_api.audio"
        ];
      };
      yandex-music = final.buildPythonPackage {
        pname = "yandex-music";
        version = "3.2.2";
        pyproject = true;
        src = final.fetchPypi {
          pname = "yandex_music";
          version = "3.2.2";
          hash = "sha256-6wtCQFIc4tBP/4dsna2n7OZUWTeZe6Eg/PqoE++CVrI=";
        };
        build-system = [ final.hatchling ];
        dependencies = [
          final.requests
          final.pysocks
          final.typing-extensions
        ];
        pythonImportsCheck = [ "yandex_music" ];
      };
    };
  };
  importer = python.withPackages (
    p:
    [
      p.vk-api
      p.beautifulsoup4
      p.yandex-music
      p.ytmusicapi
      p.yt-dlp
      p.pycryptodomex
      p.spotipy
    ]
    ++ lib.optionals (!python.stdenv.hostPlatform.isDarwin) [ p.secretstorage ]
  );
  source = lib.cleanSourceWith {
    name = "hopea-source";
    src = ../.;
    filter =
      path: type:
      let
        relative = lib.removePrefix (toString ../. + "/") path;
        parts = lib.splitString "/" relative;
        top = builtins.head parts;
        name = baseNameOf path;
      in
      builtins.elem top [
        "Cargo.toml"
        "Cargo.lock"
        "meson.build"
        "meson_options.txt"
        "flake.nix"
        "flake.lock"
        "nix"
        "COPYING"
        "LICENSES"
        "REUSE.toml"
        "ADDITIONAL-TERMS.md"
        "README.md"
        "AGENTS.md"
        ".github"
        "packaging"
        "docs"
        "src"
        "common"
        "node"
        "spotify"
        "vendor"
        "data"
        "po"
        "build-aux"
        "tests"
      ]
      && type != "symlink"
      && !(lib.any (
        part:
        (lib.hasPrefix "." part && part != ".github")
        || builtins.elem part [
          "target"
          "__pycache__"
          "node_modules"
          "result"
          "dist"
          "build"
        ]
      ) parts)
      && relative != "src/config.rs"
      && !(lib.any (suffix: lib.hasSuffix suffix name) [
        ".pyc"
        ".pyo"
        ".log"
        ".bak"
        ".flatpak"
        ".apk"
        ".zip"
        ".db"
        ".sqlite"
      ]);
  };
in
stdenv.mkDerivation {
  pname = "hopea";
  version = "2026.2";
  src = source;
  cargoDeps = rustPlatform.importCargoLock { lockFile = ../Cargo.lock; };
  postPatch = ''
    patchShebangs build-aux
  '';
  mesonFlags = [ "-Dprofile=default" ];
  # Vendoring is set up by cargoSetupHook in the parent .cargo/config.toml.
  # No dependency resolution or package installation at runtime.
  CARGO_NET_OFFLINE = "true";
  nativeBuildInputs = [
    blueprint-compiler
    cargo
    desktop-file-utils
    gettext
    m4
    meson
    ninja
    pkg-config
    python3
    rustc
    rustPlatform.cargoSetupHook
    wrapGAppsHook4
  ];
  buildInputs = [
    glib
    glib-networking
    gtk4
    libadwaita
    libsoup_3
    openssl
    adwaita-icon-theme
    hicolor-icon-theme
    gsettings-desktop-schemas
  ]
  ++ lib.optionals (!stdenv.hostPlatform.isDarwin) [ dbus ]
  ++ (with gst_all_1; [
    gstreamer
    gst-plugins-base
    gst-plugins-good
    gst-plugins-bad
    gst-plugins-ugly
    gst-libav
  ]);
  preFixup = ''
    gappsWrapperArgs+=(
      --set HOPEA_IMPORT_PYTHON "${importer}/bin/python3"
      --prefix PATH : "${lib.makeBinPath [ ffmpeg ]}"
      --prefix XDG_DATA_DIRS : "${
        lib.makeSearchPath "share" [
          adwaita-icon-theme
          hicolor-icon-theme
        ]
      }"
    )
  '';
  passthru = { inherit importer; };
  meta = {
    description = "Music player with peer-to-peer music sharing";
    license = lib.licenses.gpl3Plus;
    mainProgram = "hopea";
    platforms = lib.platforms.linux ++ lib.platforms.darwin ++ lib.platforms.freebsd;
  };
}
