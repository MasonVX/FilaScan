# FilaScan

FilaScan is firmware for a standalone Bambu Lab filament spool reader. It runs
on a WT32-SC01 Plus with a PN532 RFID reader and shows the spool information on
the integrated display immediately after a factory tag is scanned.

FilaScan is derived from the original
[yanshay/SpoolEase](https://github.com/yanshay/SpoolEase) implementation through
the intermediate
[mybesttools/SpoolEase](https://github.com/mybesttools/SpoolEase) fork. It keeps
the hardware configuration, display driver, Wi-Fi provisioning foundation and
Bambu RFID key derivation. The filament inventory, printer and AMS integration,
MQTT client, spool scale, tag writing, print analysis and SpoolEase web
applications have been removed.

FilaScan is an independent community project. It is not affiliated with or
endorsed by Bambu Lab or the SpoolEase maintainers.

## Current functionality

FilaScan reads Bambu Lab factory MIFARE Classic 1K tags. It does not modify the
tag.

The device display shows:

- official Bambu material/product name
- filament type, material ID and variant ID
- official Bambu color name and Bambu color code
- raw RGBA color value
- a large color preview, including a second color when present
- nominal filament weight
- filament diameter and length
- spool width
- nozzle and bed temperatures
- drying temperature and duration
- production date
- physical spool UID and RFID tag UID

The web interface includes a live diagnostic log for RFID detection, retries,
read failures and successful spool mappings. The in-memory log retains the most
recent 120 lines and is cleared when the device restarts.

The tag stores raw identifiers and values. FilaScan maps the material ID to a
Bambu product name using
[`core/data/base-filaments-index.csv`](core/data/base-filaments-index.csv) and
maps the material ID plus color value to a Bambu color name and code using
[`core/data/bambu-color-names.csv`](core/data/bambu-color-names.csv). Unknown
entries remain readable through their raw material ID and hexadecimal color
value.

FilaScan currently has no filament inventory and no external integration API.
Spoolman support will be implemented as a separate integration later.

## Hardware

Supported hardware:

- WT32-SC01 Plus with ESP32-S3 and 16 MB flash
- PN532 RFID reader connected over SPI
- ESP32-S3 USB JTAG/serial interface for flashing

### PN532 wiring

| PN532 signal | ESP32-S3 GPIO |
|---|---:|
| IRQ | 14 |
| SCK | 13 |
| MOSI | 11 |
| MISO | 12 |
| CS | 10 |

The PN532 SPI interface uses mode 0 at 2 MHz. Display, touch and board wiring
follow the original
[SpoolEase Console hardware documentation](https://docs.spoolease.io/docs/build-setup/console-build).

## Web interface

The web interface provides Wi-Fi configuration and a read-only live diagnostic
log. It contains no inventory, printer, MQTT, scale, OTA or
filament-management settings.

When no Wi-Fi credentials are stored, FilaScan creates an access point named
`FilaScan`. Connect to it and open:

```text
http://192.168.2.1/config
```

The display shows the temporary setup key. Enter that key on the configuration
page, enter the Wi-Fi SSID and password, then select **Save and restart**.

Wi-Fi credentials are stored in device flash. The configuration page remains
available through the device's local network address after provisioning.

## Building on macOS

Requirements:

- macOS; the current setup was tested on Apple Silicon
- [Homebrew](https://brew.sh/)
- the device connected over USB

Install the build tools and the pinned Espressif Rust toolchain:

```bash
./scripts/bootstrap-macos.sh
```

Build the release firmware:

```bash
./scripts/build-firmware.sh
```

The merged flash image is written to:

```text
build/FilaScan-esp32s3.bin
```

Flash the connected device:

```bash
./scripts/flash-device.sh /dev/cu.usbmodem31101
```

If no port is provided, the flash script uses `/dev/cu.usbmodem31101`.

## Continuous integration

The workflow in
[`firmware.yml`](.github/workflows/firmware.yml) builds the ESP32-S3 release
firmware on pushes to `main`, pull requests and manual runs. It uploads the
merged binary, its SHA-256 checksum and build metadata as a GitHub Actions
artifact. The workflow has read-only repository permissions and does not use
repository secrets.

## Repository structure

| Path | Purpose |
|---|---|
| `core/src/bambu_spool.rs` | Bambu tag parsing and local name mapping |
| `core/src/diagnostics.rs` | Bounded in-memory diagnostic log |
| `core/ui/` | Slint display UI |
| `core/static/` | Wi-Fi-only web configuration |
| `shared/src/` | PN532 reader, MIFARE access and Bambu key derivation |
| `scripts/` | macOS bootstrap, firmware build and flashing |

## AI-assisted development

OpenAI Codex was used to assist with source analysis, implementation, build
configuration and documentation. Changes are reviewed through the same build
and hardware testing process as other contributions.

## References

- [yanshay/SpoolEase](https://github.com/yanshay/SpoolEase)
- [mybesttools/SpoolEase](https://github.com/mybesttools/SpoolEase)
- [Bambu Research Group RFID Tag Guide](https://github.com/Bambu-Research-Group/RFID-Tag-Guide)
- [NXP PN532](https://www.nxp.com/products/rfid-nfc/nfc-hf/nfc-readers/standard-performance-mifare-and-ntag-frontend:PN5321A3HN)
- [Spoolman](https://github.com/Donkie/Spoolman)

## License

FilaScan retains the inherited Apache License 2.0 with Commons Clause terms.
See [`LICENSE.md`](LICENSE.md).
