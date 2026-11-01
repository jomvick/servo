#!/usr/bin/env bash
set -euo pipefail
# Ubuntu 24.04 build and software-rendering prerequisites (Servo + Komet's GPUI).
if [[ ${EUID} -eq 0 ]]; then SUDO=(); else SUDO=(sudo); fi
"${SUDO[@]}" apt-get update
"${SUDO[@]}" apt-get install -y build-essential pkg-config clang lld llvm-dev cmake ninja-build \
  libfontconfig1-dev libfreetype6-dev libssl-dev libglib2.0-dev libx11-dev libx11-xcb-dev \
  libxcb1-dev libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libegl1-mesa-dev \
  libgles2-mesa-dev libdbus-1-dev libudev-dev libvulkan-dev libasound2-dev libunwind-dev \
  gperf libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev mesa-vulkan-drivers xvfb
