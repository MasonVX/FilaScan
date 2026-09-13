#!/usr/bin/env bash
set -euo pipefail

repo_dir="$(cd "$(dirname "$0")/.." && pwd)"
output_dir="${1:-$repo_dir/build}"
if [[ "$output_dir" != /* ]]; then
  output_dir="$repo_dir/$output_dir"
fi

for export_file in "$HOME/export-esp190.sh" "$HOME/export-esp1.sh" "$HOME/export-esp.sh"; do
  if [[ -f "$export_file" ]]; then
    # shellcheck disable=SC1090
    source "$export_file"
    break
  fi
done

if command -v brew >/dev/null 2>&1; then
  rustup_bin="$(brew --prefix rustup)/bin"
  export PATH="$rustup_bin:$HOME/.cargo/bin:$PATH"
else
  export PATH="$HOME/.cargo/bin:$PATH"
fi
cd "$repo_dir/core"
cargo build --locked --release

"$repo_dir/scripts/package-firmware.sh" "$output_dir"
