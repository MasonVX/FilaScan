#!/usr/bin/env bash
set -euo pipefail

repo_dir="$(cd "$(dirname "$0")/.." && pwd)"
port="$("$repo_dir/scripts/detect-device-port.sh" "${1:-}")"

if command -v brew >/dev/null 2>&1; then
  rustup_bin="$(brew --prefix rustup)/bin"
  export PATH="$rustup_bin:$HOME/.cargo/bin:$PATH"
else
  export PATH="$HOME/.cargo/bin:$PATH"
fi

elf="$repo_dir/core/target/xtensa-esp32s3-none-elf/release/FilaScan"
elf_args=()
if [[ -f "$elf" ]]; then
  elf_args=(--elf "$elf")
fi

echo "Monitoring $port without resetting FilaScan. Press Ctrl-C to stop." >&2
exec espflash monitor \
  --chip esp32s3 \
  --port "$port" \
  --non-interactive \
  --no-reset \
  "${elf_args[@]}"
