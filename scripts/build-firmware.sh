#!/usr/bin/env bash
set -euo pipefail

repo_dir="$(cd "$(dirname "$0")/.." && pwd)"

for export_file in "$HOME/export-esp190.sh" "$HOME/export-esp1.sh" "$HOME/export-esp.sh"; do
  if [[ -f "$export_file" ]]; then
    # shellcheck disable=SC1090
    source "$export_file"
    break
  fi
done

rustup_bin="$(brew --prefix rustup)/bin"
export PATH="$rustup_bin:$HOME/.cargo/bin:$PATH"
cd "$repo_dir/core"
cargo build --locked --release

mkdir -p "$repo_dir/build"
espflash save-image \
  --chip esp32s3 \
  --flash-size 16mb \
  --flash-mode dio \
  --flash-freq 80mhz \
  --partition-table partitions.csv \
  --merge \
  target/xtensa-esp32s3-none-elf/release/FilaScan \
  "$repo_dir/build/FilaScan-esp32s3.bin"

echo "Built $repo_dir/build/FilaScan-esp32s3.bin"
