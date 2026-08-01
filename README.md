# FilaScan

FilaScan is firmware for a standalone filament RFID reader using a WT32-SC01
Plus and a PN532. It reads Bambu Lab factory tags and displays the material,
variant, color, and nominal spool weight.

The latest scan is available through an HTTP API. A bridge can send the data to
[Spoolman](https://github.com/Donkie/Spoolman).

FilaScan is based on the original [SpoolEase](https://github.com/yanshay/SpoolEase)
implementation. It uses the SpoolEase hardware support, display stack, PN532
driver, RFID decoding, and Wi-Fi provisioning. FilaScan changes the application
to reader-only operation: it does not connect to a printer over MQTT and does
not use the embedded inventory for Bambu factory-tag scans.

> [!NOTE]
> FilaScan is an independent community project. It is not affiliated with,
> endorsed by, or maintained by Bambu Lab or the SpoolEase maintainers.

FilaScan currently supports **Bambu Lab factory RFID tags**. It only reads tags
and does not modify their contents.

## Current features

- Immediate on-device display of:
  - material and material ID
  - filament variant
  - color name and hexadecimal color value
  - nominal spool weight
- Physical-spool identification using the shared spool UID stored on both
  factory tags
- Reader-only mode with all Bambu printer MQTT integrations disabled
- Upstream OTA checks disabled, preventing a FilaScan installation from being
  replaced by SpoolEase firmware
- Read-only HTTP endpoint containing the latest scan
- Optional Spoolman synchronization bridge
- Native, reproducible macOS build and flash scripts
- Original WT32-SC01 Plus display and PN532 wiring preserved

## Hardware

The currently supported device is the original SpoolEase Console hardware:

- **MCU/display:** WT32-SC01 Plus with ESP32-S3 and 16 MB flash
- **RFID reader:** PN532 connected over SPI
- **USB:** ESP32-S3 USB JTAG/serial interface for flashing and monitoring

### PN532 wiring

FilaScan preserves the upstream pin assignment:

| PN532 signal | ESP32-S3 GPIO |
|---|---:|
| IRQ | 14 |
| SCK | 13 |
| MOSI | 11 |
| MISO | 12 |
| CS | 10 |

The SPI interface runs in mode 0 at 2 MHz. Display, touch, SD-card, and power
wiring are unchanged from the upstream SpoolEase Console. Refer to the
[SpoolEase Console build documentation](https://docs.spoolease.io/docs/build-setup/console-build)
for the original assembly instructions.

## Building on macOS

### Prerequisites

- macOS (tested on Apple Silicon)
- [Homebrew](https://brew.sh/)
- A WT32-SC01 Plus connected over USB

The bootstrap script installs `rustup`, CMake, Ninja, and `pkgconf` with
Homebrew, then installs `espup` and `espflash` with Cargo. It also installs the
pinned Espressif Rust toolchain required by the upstream dependencies.

```bash
./scripts/bootstrap-macos.sh
```

FilaScan currently pins the Espressif toolchain to `1.90.0.0`. Newer toolchains
are not compatible with the upstream `esp-wifi-sys 0.8.1` dependency.

### Build the firmware

```bash
./scripts/build-firmware.sh
```

The build produces a merged 16 MB flash image at:

```text
build/bambu-rfid-reader.bin
```

The upstream executable is still named `SpoolEase` internally. This is an
implementation detail and does not affect the build or flash procedure.

### Flash the device

Find the serial device if necessary:

```bash
ls /dev/cu.usbmodem*
```

Then flash the firmware:

```bash
./scripts/flash-device.sh /dev/cu.usbmodem31101
```

The script defaults to `/dev/cu.usbmodem31101` when no port is supplied. It
does not perform a full flash erase, so existing Wi-Fi/NVS settings are normally
preserved.

For first-time Wi-Fi provisioning, use the original
[SpoolEase Console setup instructions](https://docs.spoolease.io/docs/build-setup/console-setup).

## Reader API

The latest successful scan is available on the device at:

```http
GET /api/reader/last-scan
```

Before the first scan, the endpoint returns:

```json
null
```

Example response after scanning a spool:

```json
{
  "sequence": 1,
  "tag_type": "Bambu Lab",
  "tag_id": "A1B2C3D4",
  "spool_uid": "0102030405060708",
  "vendor": "Bambu Lab",
  "material_id": "GFA00",
  "material": "PLA",
  "variant": "Basic",
  "color_name": "Jade White (#FFFFFF)",
  "color_hex": "FFFFFFFF",
  "nominal_weight_g": 1000
}
```

`sequence` increases for every successful scan. `tag_id` identifies the
individual RFID chip, while `spool_uid` identifies the physical spool and is
shared by the two factory tags attached to it.

Example request:

```bash
curl http://filascan.local/api/reader/last-scan
```

The actual hostname or IP address depends on the local network configuration.

### API security

The endpoint is intentionally read-only and unauthenticated. It exposes scan
metadata only; it does not expose Wi-Fi credentials, printer credentials,
inventory records, or the device security key.

Use FilaScan only on a trusted local network. Do not expose the device directly
to the internet or forward its HTTP port from a router.

## Spoolman integration

The bridge in [`integrations/spoolman`](integrations/spoolman) polls FilaScan
and synchronizes new scans through the official Spoolman REST API.

By default it creates missing Bambu Lab vendor and filament definitions only.
Automatic creation of physical spool records is opt-in. When enabled, the
bridge uses `spool_uid` to avoid creating two records for the two RFID tags on
one spool.

### Run with Python

The bridge uses only the Python standard library:

```bash
READER_URL=http://filascan.local \
SPOOLMAN_URL=http://spoolman.local:7912 \
python3 integrations/spoolman/spoolman_bridge.py
```

Available environment variables:

| Variable | Required | Default | Description |
|---|:---:|---|---|
| `READER_URL` | yes | — | Base URL of the FilaScan device |
| `SPOOLMAN_URL` | yes | — | Base URL of the Spoolman server |
| `POLL_INTERVAL_SECONDS` | no | `1` | Delay between reader polls |
| `SPOOLMAN_CREATE_SPOOL` | no | `false` | Create physical spool records |
| `LOG_LEVEL` | no | `INFO` | Python logging level |

To opt in to physical spool creation:

```bash
SPOOLMAN_CREATE_SPOOL=true \
READER_URL=http://filascan.local \
SPOOLMAN_URL=http://spoolman.local:7912 \
python3 integrations/spoolman/spoolman_bridge.py
```

### Run with Docker Compose

```bash
cd integrations/spoolman
cp .env.example .env
# Edit READER_URL and SPOOLMAN_URL in .env.
docker compose up -d --build
```

The bridge host must be able to reach both FilaScan and Spoolman. If the reader
is connected over Wi-Fi and the bridge runs on a wired host, ensure that the
router permits communication between those network segments and that wireless
client isolation is disabled.

## Repository layout

| Path | Purpose |
|---|---|
| `core/` | ESP32-S3 firmware and Slint device UI |
| `shared/` | Shared NFC, networking, and device components inherited from SpoolEase |
| `integrations/spoolman/` | Optional FilaScan-to-Spoolman bridge |
| `scripts/` | macOS bootstrap, build, and flash commands |
| `docs/` | Upstream web assets and flashing support |

## Origin

FilaScan is derived from these repositories:

1. **[yanshay/SpoolEase](https://github.com/yanshay/SpoolEase)** — the original
   SpoolEase project. It provides the console hardware design, ESP32 firmware
   architecture, display UI foundation, PN532 support, RFID decoding, Wi-Fi
   provisioning, and the original filament-management system.
2. **[mybesttools/SpoolEase](https://github.com/mybesttools/SpoolEase)** — the
   intermediate community fork from which FilaScan was created. It adds
   multilingual UI and web pages, translation tooling, CI/CD improvements, and
   inventory-related fixes on top of the original project.

FilaScan adds reader-only operation, a scan screen, the HTTP reader API, and the
Spoolman bridge. Printer MQTT connections and upstream OTA checks are disabled.
Bambu factory-tag scans do not create records in the embedded inventory.

Additional references:

- [SpoolEase documentation](https://docs.spoolease.io/docs/welcome)
- [Spoolman](https://github.com/Donkie/Spoolman)
- [Bambu Research Group RFID Tag Guide](https://github.com/Bambu-Research-Group/RFID-Tag-Guide)
- [NXP PN532 documentation](https://www.nxp.com/products/rfid-nfc/nfc-hf/nfc-readers/standard-performance-mifare-and-ntag-frontend:PN5321A3HN)

## Scope

FilaScan covers RFID tag reading, on-device identification, and export to
external systems. Printer control, AMS management, print monitoring, and a
separate on-device inventory are outside its scope.

## License

FilaScan is distributed under the inherited **Apache License 2.0 with Commons
Clause** terms. See [`LICENSE.md`](LICENSE.md) for the complete license and
commercial-use restrictions.

Unless stated otherwise, contributions submitted to this repository are made
under the same license terms.
