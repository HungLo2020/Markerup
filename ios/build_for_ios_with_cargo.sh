#!/usr/bin/env bash
set -euo pipefail

binary_name="${1:?missing cargo binary name}"
repo_root="$(cd "$(dirname "$0")/.." && pwd)"
target="aarch64-apple-ios"
platform="${PLATFORM_NAME:-} ${SDK_NAME:-} ${EFFECTIVE_PLATFORM_NAME:-}"
if [[ "$platform" == *simulator* ]]; then target="aarch64-apple-ios-sim"; fi

configuration="${CONFIGURATION:-Debug}"
case "$configuration" in
  Debug)
    cargo_profile="debug"
    cargo_args=()
    ;;
  Release)
    cargo_profile="release"
    cargo_args=(--release)
    ;;
  *)
    echo "Unsupported Xcode configuration: $configuration" >&2
    exit 1
    ;;
esac

cargo build \
  --manifest-path "$repo_root/Cargo.toml" \
  --target "$target" \
  --bin "$binary_name" \
  "${cargo_args[@]}"

executable_path="$BUILT_PRODUCTS_DIR/$EXECUTABLE_PATH"
rust_executable="$repo_root/target/$target/$cargo_profile/$binary_name"
if [[ ! -f "$rust_executable" ]]; then
  echo "Rust $configuration executable was not produced at: $rust_executable" >&2
  exit 1
fi
mkdir -p "$(dirname "$executable_path")"
cp "$rust_executable" "$executable_path"
chmod +x "$executable_path"
