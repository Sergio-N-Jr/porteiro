#!/usr/bin/env bash
# Gera os PNGs do ícone a partir de src-tauri/icons/icon.svg (precisa de rsvg-convert).
set -euo pipefail
cd "$(dirname "$0")/../src-tauri/icons"
rsvg-convert -w 32 -h 32 icon.svg -o 32x32.png
rsvg-convert -w 64 -h 64 icon.svg -o 64x64.png
rsvg-convert -w 128 -h 128 icon.svg -o 128x128.png
rsvg-convert -w 256 -h 256 icon.svg -o 128x128@2x.png
rsvg-convert -w 512 -h 512 icon.svg -o icon.png
echo "ícones gerados em $(pwd)"
