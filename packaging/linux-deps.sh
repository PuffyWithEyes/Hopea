#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
# CI container only: install build prerequisites in a disposable OS image.
set -euo pipefail
[[ ${HOPEA_CI_CONTAINER:-} == 1 ]] || { echo 'Run only in a disposable build container.' >&2; exit 2; }
case ${1:-} in
  debian|ubuntu)
    export DEBIAN_FRONTEND=noninteractive
    apt-get update
    apt-get install -y --no-install-recommends ca-certificates git build-essential cargo rustc \
      meson ninja-build pkg-config blueprint-compiler gettext desktop-file-utils \
      libgtk-4-dev libadwaita-1-dev libsoup-3.0-dev libssl-dev libgstreamer1.0-dev \
      libgstreamer-plugins-base1.0-dev libgstreamer-plugins-bad1.0-dev \
      python3-pip python3-venv debhelper dpkg-dev file
    ;;
  arch)
    pacman -Syu --noconfirm --needed base-devel ca-certificates git rust meson ninja pkgconf \
      blueprint-compiler gettext desktop-file-utils gtk4 libadwaita libsoup3 openssl \
      gst-plugins-base gst-plugins-good gst-plugins-bad gst-plugins-ugly gst-libav \
      python python-pip ffmpeg adwaita-icon-theme
    ;;
  fedora)
    dnf install -y ca-certificates git gcc gcc-c++ cargo rust meson ninja-build pkgconf-pkg-config \
      blueprint-compiler gettext desktop-file-utils gtk4-devel libadwaita-devel libsoup3-devel \
      openssl-devel gstreamer1-devel gstreamer1-plugins-base-devel gstreamer1-plugins-bad-free-devel \
      python3-pip rpm-build file
    ;;
  opensuse)
    zypper --non-interactive refresh
    zypper --non-interactive install ca-certificates git gcc gcc-c++ cargo rust meson ninja pkg-config \
      blueprint-compiler gettext-tools desktop-file-utils gtk4-devel libadwaita-devel libsoup-devel \
      libopenssl-devel gstreamer-devel gstreamer-plugins-base-devel gstreamer-plugins-bad-devel \
      python3-pip rpm-build file
    # python-base/pip omit sqlite3; install the matching full interpreter.
    python_package=$(python3 -c 'import sys; print("python%d%d" % sys.version_info[:2])')
    zypper --non-interactive install "$python_package"
    python3 -I -c 'import ssl, sqlite3, venv'
    ;;
  alt)
    apt-get update
    apt-get install -y ca-certificates git gcc gcc-c++ rust meson ninja-build pkg-config \
      blueprint-compiler gettext-tools desktop-file-utils libgtk4-devel libadwaita-devel \
      libsoup3.0-devel libssl-devel gstreamer1.0-devel gst-plugins1.0-devel \
      gst-plugins-bad1.0-devel python3-base python3-module-pip python3-modules-sqlite3 rpm-build file shadow-utils util-linux
    ;;
  *) echo 'Supported containers: debian ubuntu arch fedora opensuse alt' >&2; exit 2;;
esac
# Fail clearly before starting a lengthy Rust build; never weaken version checks.
pkg-config --atleast-version=4.20 gtk4
pkg-config --atleast-version=1.8 libadwaita-1
python3 -I -c 'import ssl, sqlite3, venv'
