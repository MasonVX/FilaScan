# FilaScan repository instructions

FilaScan is ESP32-S3 firmware for a standalone Bambu Lab filament RFID reader.
The device reads factory tags and immediately displays the mapped Bambu product,
color and physical spool parameters.

FilaScan does not contain a filament inventory, printer or AMS integration,
MQTT client, spool scale, tag writer, print monitor or external integration API.
The web interface is limited to Wi-Fi provisioning and read-only live
diagnostics.

## Origin

The repository is derived from `yanshay/SpoolEase` through
`mybesttools/SpoolEase`. The retained code covers the WT32-SC01 Plus hardware
foundation, PN532 communication, Bambu key derivation and Wi-Fi provisioning.

## Build

- Target: `xtensa-esp32s3-none-elf`
- Toolchain: `esp190`, Espressif Rust `1.90.0.0`
- Host tools: `espup 0.17.1`, `espflash 4.5.0`
- Hardware: WT32-SC01 Plus with 16 MB flash and PN532 over SPI
- Rust package and ELF name: `FilaScan`

Use the repository scripts:

```sh
./scripts/bootstrap-macos.sh
./scripts/build-firmware.sh
./scripts/flash-device.sh /dev/cu.usbmodem31101
```

The ELF is written to
`core/target/xtensa-esp32s3-none-elf/release/FilaScan`. The merged image is
`build/FilaScan-esp32s3.bin`.

## Relevant code

| Path | Purpose |
|---|---|
| `core/src/bambu_spool.rs` | Raw tag parsing and official-name mapping |
| `core/src/diagnostics.rs` | Bounded in-memory reader log |
| `core/src/app.rs` | Reader events and Slint state updates |
| `core/ui/` | On-device spool overview |
| `core/static/` | Wi-Fi-only web configuration |
| `shared/src/bambu_reader.rs` | Continuous PN532 scan loop |
| `shared/src/nfc.rs` | Required Bambu tag blocks |
| `shared/src/pn532_ext.rs` | MIFARE reads and Bambu key derivation |
| `.github/workflows/firmware.yml` | Reproducible CI firmware build |

## Constraints

- Preserve the WT32-SC01 Plus and PN532 pin assignment unless a new board target
  is introduced explicitly.
- Keep RFID operation read-only.
- Preserve partial Bambu payload blocks across retries; marginal RF coupling
  must not force already-read sectors to be fetched again.
- After a MIFARE read failure, return to `InListPassiveTarget`; do not add
  `InRelease`/`InSelect` retries that bypass the original SpoolEase reader flow.
- Keep material and color mapping local and retain raw values as the fallback
  for unknown Bambu entries.
- Do not add inventory, printer, MQTT, scale or OTA controls to the Wi-Fi and
  diagnostics page.
- Add an external integration API as a separate, deliberately reviewed change.
