# FilaScan repository instructions

FilaScan is ESP32-S3 firmware for a standalone filament RFID/NFC reader. It
reads supported Bambu Lab and OpenPrintTag spools and immediately displays the
identified product, material, color, and physical spool parameters.

FilaScan is not an authoritative filament inventory, printer controller, AMS
controller, MQTT client, spool scale, tag writer, or print monitor. It can cache
a read-only FilaMan inventory snapshot and queue location choices on SD while
offline. The web interface provides Wi-Fi, catalog, FilaMan, and OTA
configuration plus read-only live diagnostics.

## Origin

The repository is derived from `yanshay/SpoolEase` through
`mybesttools/SpoolEase`. The retained code covers the WT32-SC01 Plus hardware
foundation, PN532 communication, Bambu key derivation, and Wi-Fi provisioning.
FilaScan also supports a PN5180 reader and OpenPrintTag through separate reader
and tag-format implementations.

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
./scripts/release-preflight.sh <version>
```

The ELF is written to
`core/target/xtensa-esp32s3-none-elf/release/FilaScan`. Firmware artifacts are
written to `build/`. Flashing rebuilds the firmware and refuses uncommitted
firmware sources. Monitoring attaches without resetting the board.

## Relevant code

| Path | Purpose |
|---|---|
| `core/src/bambu_spool.rs` | Bambu tag parsing and product mapping |
| `core/src/openprinttag_catalog.rs` | OpenPrintTag product enrichment and metadata cache |
| `core/src/filaman.rs` | FilaMan transport and synchronization orchestration |
| `core/src/filaman/offline.rs` | Cached inventory and offline operation queue |
| `core/src/diagnostics.rs` | Bounded in-memory reader log |
| `core/src/app.rs` | Reader events and Slint state updates |
| `core/src/ota.rs` | GitHub-hosted update checks and OTA installation |
| `core/ui/` | On-device spool overview, actions, and firmware status |
| `core/static/` | Protected Wi-Fi, catalog, FilaMan, and OTA configuration |
| `formats/src/openprinttag.rs` | OpenPrintTag NDEF/CBOR decoding |
| `shared/src/bambu_reader.rs` | Reader selection and hardware-neutral events |
| `shared/src/pn532_reader.rs` | PN532 scan and recovery loop |
| `shared/src/pn532_ext.rs` | PN532 MIFARE block adapter |
| `shared/src/pn5180.rs` | PN5180 command, ISO-A, and NFC-V driver |
| `shared/src/pn5180_reader.rs` | PN5180 scan and recovery loop |
| `shared/src/nfc.rs` | Bambu key derivation and required tag blocks |
| `.github/workflows/firmware.yml` | Reproducible CI firmware build and release |

## Constraints

- Preserve the WT32-SC01 Plus reader pin assignment and automatic
  PN5180-to-PN532 selection unless a new board target is introduced explicitly.
- Keep RFID/NFC operation read-only.
- Preserve partial Bambu payload blocks across retries; marginal RF coupling
  must not force already-read sectors to be fetched again.
- After a PN532 MIFARE read failure, return to `InListPassiveTarget`; do not add
  `InRelease`/`InSelect` retries that bypass the original SpoolEase reader flow.
- Keep reader-specific behavior behind the hardware-neutral `ReaderEvent`
  interface. Shared Bambu tag knowledge belongs in `shared/src/nfc.rs`; tag
  format decoding must remain independent from a particular reader backend.
- Keep product enrichment separate from tag decoding. Tag values take
  precedence, and unknown products must retain useful raw-value fallbacks.
- Keep the FilaMan server authoritative. Offline state is a cached snapshot and
  a durable queue, not a second inventory database. Avoid unnecessary SD writes.
- Do not add printer, MQTT, scale, or tag-writing controls to the configuration
  page. OTA installation must remain explicit and user initiated.
- Preserve unrelated worktree changes. Commit relevant firmware changes before
  flashing, and never move or replace an existing release tag without explicit
  user direction.
