#!/usr/bin/env bash
set -euo pipefail

brew install rustup cmake ninja pkgconf

RUSTUP_BIN="$(brew --prefix rustup)/bin"
export PATH="$RUSTUP_BIN:$HOME/.cargo/bin:$PATH"

rustup default stable
cargo install espup --locked
cargo install espflash --locked

if ! rustup toolchain list | grep -q '^esp190'; then
  espup install \
    --name esp190 \
    --toolchain-version 1.90.0.0 \
    --targets esp32s3 \
    --export-file "$HOME/export-esp190.sh"
fi

echo "Toolchain ready. Source ~/export-esp190.sh, then run scripts/build-firmware.sh."
