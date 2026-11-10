#!/usr/bin/env bash
# Ubuntu 24.04 does not package gtk4-layer-shell; build it from source.
# Installs both the shared and the static library under /usr/local.
set -euxo pipefail
version="${1:-v1.3.0}"
sudo apt-get update
sudo apt-get install -y libgtk-4-dev libwayland-dev wayland-protocols meson ninja-build
git clone --depth 1 --branch "$version" https://github.com/wmww/gtk4-layer-shell /tmp/gtk4-layer-shell
cd /tmp/gtk4-layer-shell
meson setup build --prefix=/usr/local -Ddefault_library=both -Dintrospection=false -Dvapi=false -Dexamples=false -Ddocs=false -Dtests=false
ninja -C build
sudo ninja -C build install
sudo ldconfig
