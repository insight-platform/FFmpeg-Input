#!/usr/bin/env bash

set -e

apt-get update && apt-get -y install \
    liboping0 \
    liboping-dev \
    clang \
    autoconf \
    automake \
    build-essential \
    cmake \
    git-core \
    libass-dev \
    libavutil55 libavutil-dev \
    libavformat57 libavformat-dev \
    libavfilter6 libavfilter-dev \
    libavdevice57 libavdevice-dev \
    libfreetype6-dev \
    libgnutls28-dev \
    libmp3lame-dev \
    libsdl2-dev \
    libtool \
    libva-dev \
    libvdpau-dev \
    libvorbis-dev \
    libxcb1-dev \
    libxcb-shm0-dev \
    libxcb-xfixes0-dev \
    meson \
    ninja-build \
    pkg-config \
    texinfo \
    wget \
    yasm \
    zlib1g-dev \
    openssl \
    libsasl2-dev \
    libsasl2-2 \
    python3-dev \
    python3-pip

curl -o rustup.sh --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs
sh rustup.sh -y

/usr/bin/python3 -m pip install --upgrade pip
/usr/bin/python3 -m pip install --upgrade maturin~=0.13
