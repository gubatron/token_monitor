#!/usr/bin/env bash
# Install the system libraries the windows/ workspace needs to compile on a
# Linux runner. The product targets Windows only, but lint, tests and
# coverage also run on ubuntu, and Slint's renderer pulls in -sys crates
# that link system libraries there:
#   yeslogic-fontconfig-sys  libfontconfig1-dev (+ libfreetype-dev)
#   xkbcommon / wayland      libxkbcommon-dev, libwayland-dev
#   drm-sys                  libdrm-dev
#   libudev (input backends) libudev-dev
# clang (for clang-sys / bindgen) and pkg-config are already on the image.
set -euo pipefail

if [ "$(uname -s)" != Linux ]; then
  echo "linux-deps.sh: not Linux, nothing to install."
  exit 0
fi

sudo apt-get update -qq
sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends \
  pkg-config libfontconfig1-dev libfreetype-dev libxkbcommon-dev libwayland-dev \
  libdrm-dev libudev-dev
