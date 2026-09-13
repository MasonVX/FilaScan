#!/usr/bin/env bash
set -euo pipefail

repo_dir="$(cd "$(dirname "$0")/.." && pwd)"
output_dir="${1:-$repo_dir/build}"
elf="$repo_dir/core/target/xtensa-esp32s3-none-elf/release/FilaScan"

if [[ ! -f "$elf" ]]; then
  echo "Release ELF not found. Run scripts/build-firmware.sh first." >&2
  exit 1
fi

version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$repo_dir/core/Cargo.toml" | head -1)"
if [[ -z "$version" ]]; then
  echo "Could not read the FilaScan version from core/Cargo.toml." >&2
  exit 1
fi

mkdir -p "$output_dir"
merged_file="$output_dir/FilaScan-esp32s3.bin"
ota_file="$output_dir/FilaScan-$version-ota.bin"

espflash save-image \
  --chip esp32s3 \
  --flash-size 16mb \
  --flash-mode dio \
  --flash-freq 80mhz \
  --partition-table "$repo_dir/core/partitions.csv" \
  --merge \
  "$elf" \
  "$merged_file"

espflash save-image \
  --chip esp32s3 \
  --flash-size 16mb \
  --flash-mode dio \
  --flash-freq 80mhz \
  --partition-table "$repo_dir/core/partitions.csv" \
  "$elf" \
  "$ota_file"

if command -v sha256sum >/dev/null 2>&1; then
  (cd "$output_dir" && sha256sum "$(basename "$merged_file")" "$(basename "$ota_file")" > SHA256SUMS)
else
  (cd "$output_dir" && shasum -a 256 "$(basename "$merged_file")" "$(basename "$ota_file")" > SHA256SUMS)
fi

ota_size="$(wc -c < "$ota_file" | tr -d ' ')"
ota_crc32="$(python3 -c 'import pathlib, sys, zlib; print(f"{zlib.crc32(pathlib.Path(sys.argv[1]).read_bytes()) & 0xffffffff:08x}")' "$ota_file")"
printf 'filename = "FilaScan-%s-ota.bin"\nversion = "%s"\nfilesize = %s\ncrc32 = "%s"\n' \
  "$version" "$version" "$ota_size" "$ota_crc32" > "$output_dir/ota.toml"

git_commit="$(git -C "$repo_dir" rev-parse HEAD)"
printf 'project=FilaScan\nfirmware_version=%s\ngit_commit=%s\ntarget=esp32s3\nflash_size=16mb\n' \
  "$version" "$git_commit" > "$output_dir/build-info.txt"

echo "Packaged FilaScan $version firmware in $output_dir"
