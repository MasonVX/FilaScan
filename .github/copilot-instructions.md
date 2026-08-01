# FilaScan repository instructions

FilaScan is ESP32-S3 firmware for a standalone Bambu Lab filament RFID reader.
Its primary behavior is to read factory tags, display filament data, and expose
the latest scan to external systems. Printer MQTT connections, AMS management,
print monitoring, and the embedded SpoolEase inventory are not product goals.

## Origin

The repository is derived from `yanshay/SpoolEase` through
`mybesttools/SpoolEase`. The Rust package and some internal paths still use the
upstream `SpoolEase` name. Do not rename those identifiers without checking all
build, linker, packaging, and persistence references.

## Build

- Target: `xtensa-esp32s3-none-elf`
- Toolchain: `esp190`, Espressif Rust `1.90.0.0`
- Host tools: `espup 0.17.1`, `espflash 4.5.0`
- Supported hardware: WT32-SC01 Plus with 16 MB flash and PN532 over SPI

Use the repository scripts:

```sh
./scripts/bootstrap-macos.sh
./scripts/build-firmware.sh
./scripts/flash-device.sh /dev/cu.usbmodem31101
```

The ELF is written to
`core/target/xtensa-esp32s3-none-elf/release/SpoolEase`. The merged local flash
image is `build/FilaScan-esp32s3.bin`; CI publishes the same filename.

Files under `core/static/` are embedded at compile time. Cargo does not track
all of them as dependencies. After changing static HTML, clean the application
crate before building:

```sh
cd core
cargo clean -p SpoolEase --target xtensa-esp32s3-none-elf --release
cargo build --locked --release
```

## Relevant code

| Path | Purpose |
|---|---|
| `core/src/view_model.rs` | Reader scan processing and UI state |
| `core/src/tag_standards.rs` | Bambu tag fields and spool identity |
| `core/src/web_app.rs` | Read-only reader HTTP endpoint |
| `core/ui/` | Slint device UI |
| `shared/src/nfc.rs` | RFID block-read definitions |
| `integrations/spoolman/` | Optional Spoolman synchronization bridge |
| `.github/workflows/firmware.yml` | Reproducible CI firmware build |

## Constraints

- Keep `READER_ONLY_MODE` enabled unless the product scope changes explicitly.
- Do not re-enable upstream OTA checks. They can replace FilaScan with upstream
  firmware.
- Keep `/api/reader/last-scan` read-only and free of credentials or inventory
  data.
- Physical Spoolman record creation must remain opt-in.
- Preserve the upstream hardware pin assignment unless a new board target is
  introduced explicitly.
