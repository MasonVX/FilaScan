#!/usr/bin/env bash
set -euo pipefail

repo_dir="$(cd "$(dirname "$0")/.." && pwd)"
port="$("$repo_dir/scripts/detect-device-port.sh" "${1:-}")"

dirty_sources="$(git -C "$repo_dir" status --porcelain --untracked-files=all -- core shared formats scripts/build-firmware.sh scripts/package-firmware.sh scripts/flash-device.sh)"
if [[ -n "$dirty_sources" ]]; then
  echo "Refusing to flash firmware built from uncommitted source or build-script changes:" >&2
  printf '%s\n' "$dirty_sources" >&2
  exit 1
fi

rustup_bin="$(brew --prefix rustup)/bin"
export PATH="$rustup_bin:$HOME/.cargo/bin:$PATH"
"$repo_dir/scripts/build-firmware.sh"
espflash flash \
  --chip esp32s3 \
  --flash-size 16mb \
  --flash-mode dio \
  --flash-freq 80mhz \
  --partition-table "$repo_dir/core/partitions.csv" \
  --port "$port" \
  "$repo_dir/core/target/xtensa-esp32s3-none-elf/release/FilaScan"
