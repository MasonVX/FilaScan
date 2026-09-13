# FilaScan repository instructions

FilaScan is ESP32-S3 firmware for a standalone Bambu Lab filament RFID reader.
The device reads factory tags and immediately displays the mapped Bambu product,
color and physical spool parameters.

FilaScan does not contain a local filament inventory, printer or AMS
integration, MQTT client, spool scale, tag writer or print monitor. The web
interface provides Wi-Fi, catalog, FilaMan and OTA configuration plus read-only
live diagnostics.

## Origin

The repository is derived from `yanshay/SpoolEase` through
`mybesttools/SpoolEase`. The retained code covers the WT32-SC01 Plus hardware
foundation, PN532 communication, Bambu key derivation and Wi-Fi provisioning.
FilaScan also supports a PN5180 reader through its own backend.

## Build

- Target: `xtensa-esp32s3-none-elf`
- Toolchain: `esp190`, Espressif Rust `1.90.0.0`
- Host tools: `espup 0.17.1`, `espflash 4.5.0`
- Hardware: WT32-SC01 Plus with 16 MB flash and PN532 or PN5180 over SPI
- Rust package and ELF name: `FilaScan`

Use the repository scripts:

```sh
./scripts/bootstrap-macos.sh
./scripts/build-firmware.sh
./scripts/flash-device.sh [/dev/cu.usbmodem...]
./scripts/monitor-device.sh [/dev/cu.usbmodem...]
```

The ELF is written to
`core/target/xtensa-esp32s3-none-elf/release/FilaScan`. The merged image is
`build/FilaScan-esp32s3.bin`; the application-only OTA image and its manifest
are written to the same directory. Flashing rebuilds the firmware and refuses
uncommitted firmware sources. Monitoring attaches without resetting the board.

## Relevant code

| Path | Purpose |
|---|---|
| `core/src/bambu_spool.rs` | Raw tag parsing and official-name mapping |
| `core/src/diagnostics.rs` | Bounded in-memory reader log |
| `core/src/app.rs` | Reader events and Slint state updates |
| `core/ui/` | On-device spool overview, actions and firmware status |
| `core/static/` | Protected Wi-Fi, catalog, FilaMan and OTA configuration |
| `shared/src/bambu_reader.rs` | Reader selection and hardware-neutral events |
| `shared/src/pn532_reader.rs` | PN532 scan and recovery loop |
| `shared/src/pn532_ext.rs` | PN532 MIFARE block adapter |
| `shared/src/pn5180.rs` | PN5180 command and ISO-A driver |
| `shared/src/pn5180_reader.rs` | PN5180 scan and recovery loop |
| `shared/src/nfc.rs` | Bambu key derivation and required tag blocks |
| `.github/workflows/firmware.yml` | Reproducible CI firmware build |

## Constraints

- Preserve the WT32-SC01 Plus reader pin assignment and the automatic
  PN5180-to-PN532 selection unless a new board target is introduced explicitly.
- Keep RFID operation read-only.
- Preserve partial Bambu payload blocks across retries; marginal RF coupling
  must not force already-read sectors to be fetched again.
- After a PN532 MIFARE read failure, return to `InListPassiveTarget`; do not add
  `InRelease`/`InSelect` retries that bypass the original SpoolEase reader flow.
- Keep reader-specific behavior behind the hardware-neutral `ReaderEvent`
  interface. Shared Bambu tag knowledge belongs in `shared/src/nfc.rs`.
- Keep material and color mapping local and retain raw values as the fallback
  for unknown Bambu entries.
- Do not add inventory, printer, MQTT or scale controls to the configuration
  page. Keep OTA controls limited to an explicit version check and a confirmed,
  user-initiated installation through the existing framework updater.
- Add an external integration API as a separate, deliberately reviewed change.
