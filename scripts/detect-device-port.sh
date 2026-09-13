#!/usr/bin/env bash
set -euo pipefail

requested_port="${1:-${FILASCAN_PORT:-}}"
if [[ -n "$requested_port" ]]; then
  if [[ ! -e "$requested_port" ]]; then
    echo "FilaScan serial port does not exist: $requested_port" >&2
    exit 1
  fi
  printf '%s\n' "$requested_port"
  exit 0
fi

device_dir="${FILASCAN_DEV_DIR:-/dev}"
ports=()
while IFS= read -r port; do
  ports+=("$port")
done < <(find "$device_dir" -maxdepth 1 \( -name 'cu.usbmodem*' -o -name 'cu.usbserial*' \) -print | sort)

case "${#ports[@]}" in
  0)
    echo "No USB serial device found. Connect FilaScan or pass a port explicitly." >&2
    exit 1
    ;;
  1)
    printf '%s\n' "${ports[0]}"
    ;;
  *)
    echo "Multiple USB serial devices found; pass the intended port explicitly:" >&2
    printf '  %s\n' "${ports[@]}" >&2
    exit 2
    ;;
esac
