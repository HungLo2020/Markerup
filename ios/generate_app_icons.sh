#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
source_svg="${1:-$repo_root/markerup_notepad_icon.svg}"
asset_dir="$repo_root/ios/Assets.xcassets/AppIcon.appiconset"

if ! command -v rsvg-convert >/dev/null 2>&1; then
  echo "rsvg-convert is required; install librsvg (brew install librsvg)" >&2
  exit 1
fi
test -f "$source_svg"
mkdir -p "$asset_dir"

generate() {
  local filename="$1" size="$2"
  rsvg-convert --width "$size" --height "$size" --output "$asset_dir/$filename" "$source_svg"
}

generate icon-20@2x.png 40
generate icon-20@3x.png 60
generate icon-29@2x.png 58
generate icon-29@3x.png 87
generate icon-40@2x.png 80
generate icon-40@3x.png 120
generate icon-60@2x.png 120
generate icon-60@3x.png 180
generate icon-76@2x.png 152
generate icon-83.5@2x.png 167
generate icon-1024.png 1024
