#!/usr/bin/env bash
set -euo pipefail

repo_dir="$(cd "$(dirname "$0")/.." && pwd)"
port="${1:-/dev/cu.usbmodem31101}"

export PATH="$(brew --prefix rustup)/bin:$HOME/.cargo/bin:$PATH"
espflash flash \
  --chip esp32s3 \
  --flash-size 16mb \
  --flash-mode dio \
  --flash-freq 80mhz \
  --partition-table "$repo_dir/core/partitions.csv" \
  --port "$port" \
  "$repo_dir/core/target/xtensa-esp32s3-none-elf/release/SpoolEase"
