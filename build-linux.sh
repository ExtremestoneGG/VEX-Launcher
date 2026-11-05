#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cache_root="${VEX_BUILD_CACHE_ROOT:-$root/.vex-dev-cache}"

export CARGO_HOME="${CARGO_HOME:-$cache_root/cargo}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$cache_root/cargo-target}"
export TMPDIR="${TMPDIR:-$cache_root/tmp}"
export XDG_CACHE_HOME="${XDG_CACHE_HOME:-$cache_root/runtime-cache}"
export XDG_DATA_HOME="${XDG_DATA_HOME:-$cache_root/runtime-data}"
export VEX_STORAGE_ROOT="${VEX_STORAGE_ROOT:-$cache_root/runtime-data/vex-launcher}"
export npm_config_cache="${npm_config_cache:-$cache_root/npm}"
export NO_STRIP="${NO_STRIP:-1}"

mkdir -p "$CARGO_HOME" "$CARGO_TARGET_DIR" "$TMPDIR" "$XDG_CACHE_HOME" "$XDG_DATA_HOME" "$VEX_STORAGE_ROOT" "$npm_config_cache"
cd "$root"

if [[ ! -d node_modules ]]; then
  npm ci
fi

npm run tauri -- build --bundles appimage

artifact="$(find "$CARGO_TARGET_DIR/release/bundle/appimage" -maxdepth 1 -name '*.AppImage' -print -quit)"
if [[ -z "$artifact" ]]; then
  printf 'O AppImage não foi encontrado após a compilação.\n' >&2
  exit 1
fi

sha256sum "$artifact" > "$artifact.sha256"
printf 'AppImage criado: %s\nChecksum: %s\n' "$artifact" "$artifact.sha256"
