#!/usr/bin/env bash
# Ubuntu 24.04 does not package gtk4-layer-shell; build it from source.
# Installs both the shared and the static library under /usr/local.
set -euxo pipefail
version="${1:-v1.3.0}"

# The runners' apt sources read a mirror list with Azure's Ubuntu mirror
# first. On 2026-11-11 that host stopped resolving and apt waited 20 s on
# it for every package, so this step ran past half an hour. Drop any
# mirror whose host doesn't resolve, and keep apt's waits short.
mirrors=/etc/apt/apt-mirrors.txt
if [ -f "$mirrors" ]; then
  cat "$mirrors"
  grep -E '^[a-z]+://' "$mirrors" | while read -r url _; do
    host=$(sed -E 's#^[a-z]+://([^/:]+).*#\1#' <<<"$url")
    if ! getent hosts "$host" >/dev/null; then
      echo "Dropping apt mirror $url: $host does not resolve"
      sudo sed -i "\\#^$url#d" "$mirrors"
    fi
  done
  if ! grep -qE '^[a-z]+://' "$mirrors"; then
    if [ "$(dpkg --print-architecture)" = amd64 ]; then
      echo "http://archive.ubuntu.com/ubuntu/" | sudo tee "$mirrors"
    else
      echo "http://ports.ubuntu.com/ubuntu-ports/" | sudo tee "$mirrors"
    fi
  fi
fi
apt=(sudo apt-get -o Acquire::Retries=3 -o Acquire::http::Timeout=15 -o Acquire::https::Timeout=15)
"${apt[@]}" update
"${apt[@]}" install -y libgtk-4-dev libwayland-dev wayland-protocols meson ninja-build

git clone --depth 1 --branch "$version" https://github.com/wmww/gtk4-layer-shell /tmp/gtk4-layer-shell
cd /tmp/gtk4-layer-shell
meson setup build --prefix=/usr/local -Ddefault_library=both -Dintrospection=false -Dvapi=false -Dexamples=false -Ddocs=false -Dtests=false
ninja -C build
sudo ninja -C build install
sudo ldconfig
