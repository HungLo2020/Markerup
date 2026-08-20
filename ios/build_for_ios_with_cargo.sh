#!/usr/bin/env bash
set -euo pipefail

binary_name="${1:?missing cargo binary name}"
repo_root="$(cd "$(dirname "$0")/.." && pwd)"
target="aarch64-apple-ios"
if [[ "${PLATFORM_NAME:-}" == *simulator* ]]; then target="aarch64-apple-ios-sim"; fi

cargo build --manifest-path "$repo_root/Cargo.toml" --target "$target" --bin "$binary_name"

executable_path="$BUILT_PRODUCTS_DIR/$EXECUTABLE_PATH"
mkdir -p "$(dirname "$executable_path")"
cp "$repo_root/target/$target/debug/$binary_name" "$executable_path"
chmod +x "$executable_path"
