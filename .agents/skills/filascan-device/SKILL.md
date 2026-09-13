---
name: filascan-device
description: Build, flash, monitor, or diagnose FilaScan firmware on WT32-SC01 Plus hardware. Use for local ESP32-S3 toolchain, USB-port, PN532/PN5180, boot, crash, and serial-log tasks; do not use for publishing releases.
---

# FilaScan Device

Work from the FilaScan repository root. Read `.github/copilot-instructions.md` before changing firmware or hardware behavior.

## Build

- Run `./scripts/bootstrap-macos.sh` only when required tools are missing.
- Run `./scripts/build-firmware.sh` for a release build and both USB/OTA artifacts.
- Use `FILASCAN_RFID_READER=pn532` or `pn5180` only when the user explicitly wants to override automatic reader detection.
- Report the binary path and flash utilization from the build output.

## Flash

- A user request to flash authorizes flashing the connected FilaScan board, but not another USB device.
- Preserve the project rule that every flashed binary corresponds to a commit. Commit relevant firmware and build-script changes before invoking the flash script; never include unrelated user changes.
- Run `./scripts/flash-device.sh [port]`. It rebuilds from committed sources and auto-selects the port only when exactly one supported USB serial device is present.
- If multiple ports exist, resolve the intended board with the user rather than guessing.

## Diagnose

- Prefer the existing web diagnostics while the device responds.
- For serial output, run `./scripts/monitor-device.sh [port]`. It attaches without resetting the device. Do not use a plain `espflash monitor` command because its defaults reset the target.
- State explicitly before any intentional reset, reflash, or power-cycle because it destroys the current failure state.
- Read [references/diagnostics.md](references/diagnostics.md) when investigating hangs, resets, reader failures, or intermittent behavior.
- Stop monitors after collecting enough evidence; do not leave a USB session holding the device unnecessarily.
