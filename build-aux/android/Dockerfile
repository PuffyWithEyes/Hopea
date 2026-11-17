# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
FROM debian:trixie-slim@sha256:a72b4433de1cf7d6bae0c4cee74d0547f58601a643b3375f070c51600d63b6b2
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential ca-certificates curl git unzip xz-utils file pkg-config \
    openjdk-21-jdk-headless sdkmanager python3-pip python3-venv ninja-build \
    libglib-perl libglib-object-introspection-perl libipc-run-perl libjson-perl \
    libset-scalar-perl libxml-libxml-perl libxml-libxslt-perl gir1.2-appstream \
    glslc gobject-introspection libglib2.0-dev-bin libxml2-utils sassc gettext \
    cmake bison flex gperf libssl-dev fonts-dejavu-core
RUN python3 -m venv /opt/build-tools && /opt/build-tools/bin/pip install meson==1.11.0
ENV PATH=/opt/build-tools/bin:/root/.cargo/bin:$PATH
ENV ANDROID_HOME=/opt/android
RUN sdkmanager --sdk_root=/opt/android 'build-tools;36.0.0' 'platforms;android-36' 'ndk;28.2.13676358'
RUN curl --fail --proto '=https' --tlsv1.2 https://sh.rustup.rs -o /tmp/rustup.sh && \
    sh /tmp/rustup.sh -y --profile minimal --default-toolchain 1.100.0 && \
    rustup target add aarch64-linux-android x86_64-linux-android armv7-linux-androideabi
RUN git clone https://github.com/sp1ritCS/gtk-android-builder.git /opt/pixiewood && \
    git -C /opt/pixiewood checkout de3719725a8f6e3efe0f28203e803e7ce1904902 && \
    mkdir -p /opt/mini-studio
