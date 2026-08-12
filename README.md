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
- the matching Bambu product image when Wi-Fi or an SD cache is available
- a large color preview, including a second color when present
- nominal filament weight
- filament diameter and length
- spool width
- nozzle and bed temperatures
- drying temperature and duration
- production date
- 16-byte tray UID and RFID tag UID

The display and configuration page support English and German. The selected
language is stored on the SD card. Bambu color names use the corresponding
language field from the downloaded BambuStudio catalog, with English as the
fallback when a translation is missing. Bambu product and material names such
as `PLA Matte` remain unchanged. Technical diagnostic logs remain in English.

The web interface includes a live diagnostic log for RFID detection, retries,
read failures and successful spool mappings. Every successful scan prints all
decoded fields and a hexadecimal dump of every payload block read from the tag.
Authentication keys and Wi-Fi credentials are not logged. The in-memory log
retains the most recent 120 lines and is cleared when the device restarts.

The tag stores raw identifiers and values. FilaScan downloads the official
[`filaments_color_codes.json`](https://github.com/bambulab/BambuStudio/blob/master/resources/profiles/BBL/filament/filaments_color_codes.json)
catalog directly from BambuStudio. The catalog is validated before use and
cached as `/filascan/catalog/catalog.jsn` on the SD card. It supplies the official
filament type, color name and five-digit Bambu color code. The lookup primarily
uses the RFID material ID and variant color code, with the RFID color value and
the existing compact legacy mappings as fallbacks. Unknown entries remain
readable through their raw identifiers and hexadecimal color values.

Automatic catalog updates run once every 24 hours while Wi-Fi is available.
The source URL, automatic updates and a manual update action are available on
the protected configuration page. Only direct HTTPS URLs on
`raw.githubusercontent.com` are accepted. A failed download or invalid JSON
does not replace the last valid SD-card cache.

For a recognized five-digit Bambu product code, FilaScan queries Bambu's EU
Store `globalSearchV2` API. It accepts only one exact search result and uses the
highlighted SKU's first `mediaFiles` product image, matching the resolver used
by the FilaMan Bambu Lab plugin. The selected CDN image is requested as a
240-pixel JPEG and shown above the color preview. No product-page scraping or
static product-image mapping is included in the firmware.

Store API and image downloads use HTTPS with certificate validation, restricted
Bambu hosts, response-size limits and image-dimension checks. If Wi-Fi is
unavailable, the product code is unknown or the lookup fails, the spool data
and color preview remain available without an image.

Product images are not stored in device flash. With an SD card installed,
FilaScan checks `/filascan/images/<product-code>.jpg` before contacting the
Bambu Store API. A valid cache hit is displayed without a network request and
survives restarts and firmware updates. Invalid JPEG files are ignored and
downloaded again when Wi-Fi is available. Cached images do not expire or refresh
automatically; delete the corresponding file or the complete
`/filascan/images/` directory to force a new download. Without an SD card, the
downloaded image is kept only for the current display session and the next scan
must fetch it again. Product images remain Bambu Lab content and are retrieved
at runtime; they are not included in the firmware image.

### FilaMan integration

FilaScan can add a recognized spool to a separate
[FilaMan](https://github.com/Fire-Devils/filaman-system) instance. The 16-byte
Bambu Tray UID is stored as FilaMan's unique `external_id`. The short NFC Tag
UID is not included in any FilaMan request.

After a scan, FilaScan first checks whether that Tray UID is already registered.
For an existing spool, its current location appears as a button in the spool
view. Pressing it opens the location selector; choosing another regular
location moves the existing spool and records the change through the FilaMan
plugin. The same dialog can archive an existing spool after an explicit
confirmation, allowing other integrations to remove it from their active
inventory while preserving its identity and synchronization history. For a new
spool, FilaScan opens the same selector before creating it.
Bambu AMS locations whose identifier starts with `bambulab` are excluded. The
spool is created only after the user chooses a regular storage location; Cancel
leaves FilaMan unchanged.

The confirmed import sends the decoded spool data and selected `location_id` in
one authenticated request to the FilaScan integration plugin at
`POST /api/v1/devices/filascan/import-spool?type=bambu`. The plugin owns the
transactional, idempotent creation or resolution of the manufacturer, colors,
filament and spool.

The compatible FilaMan integration plugin must be installed and the registered
FilaScan device must be authorized by that plugin. Configure the FilaMan URL
and the normal FilaMan device token on the protected web page. General FilaMan
inventory scopes are not required. FilaMan integration is disabled by default.
The connection test calls `GET /api/v1/devices/filascan/status` and verifies
that the plugin reports `ready` with `location_selection: true` and
`location_management: true` and `spool_archiving: true`.

For HTTPS, FilaScan validates the server certificate against the configured
PEM CA before sending the token or spool data. Direct HTTP URLs are also
supported for trusted local networks and do not require a CA certificate; the
token and spool data are then transmitted without transport encryption.
Settings are stored on the SD card.

Spoolman support may be implemented as a separate integration later.

## Hardware

Supported hardware:

- WT32-SC01 Plus with ESP32-S3 and 16 MB flash
- PN532 RFID reader connected over SPI
- ESP32-S3 USB JTAG/serial interface for flashing
- optional FAT-formatted microSD card for the Bambu catalog and product image caches

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

The web interface provides language selection, Wi-Fi configuration, Bambu
catalog update settings, FilaMan integration settings and a read-only live
diagnostic log. It contains no local inventory, printer, MQTT, scale, OTA or
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
The selected language, catalog settings, FilaMan settings and the last valid
downloaded catalog are stored on the SD card. Without an SD card, language
changes apply only to the current session, and a catalog can still be
downloaded into memory but cannot be retained across restarts. FilaMan
integration settings cannot be saved without an SD card.

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

Tags matching `filascan-v*` run the same reproducible build and create a GitHub
release containing the merged firmware image, SHA-256 checksum and build
metadata. Only the release job receives permission to create the release. The
FilaScan version in `core/Cargo.toml` must match the numeric part of the tag.

## Repository structure

| Path | Purpose |
|---|---|
| `core/src/bambu_spool.rs` | Bambu tag parsing and local product mapping |
| `core/src/catalog.rs` | BambuStudio catalog download, validation and SD cache |
| `core/src/filaman.rs` | FilaMan plugin HTTP(S) client and Bambu import payload |
| `core/src/image_loader.rs` | Restricted Bambu Store lookup, HTTPS image download and JPEG decoding |
| `core/src/localization.rs` | Display and web language selection with SD persistence |
| `core/src/diagnostics.rs` | Bounded in-memory diagnostic log |
| `core/ui/` | Slint display UI |
| `core/static/` | Protected Wi-Fi, catalog and FilaMan configuration |
| `shared/src/` | PN532 reader, MIFARE access and Bambu key derivation |
| `scripts/` | macOS bootstrap, firmware build and flashing |

## AI-assisted development

OpenAI Codex was used to assist with source analysis, implementation, build
configuration and documentation. Changes are reviewed through the same build
and hardware testing process as other contributions.

## References

- [yanshay/SpoolEase](https://github.com/yanshay/SpoolEase)
- [mybesttools/SpoolEase](https://github.com/mybesttools/SpoolEase)
- [BambuStudio filament color catalog](https://github.com/bambulab/BambuStudio/blob/master/resources/profiles/BBL/filament/filaments_color_codes.json)
- [Bambu Research Group RFID Tag Guide](https://github.com/Bambu-Research-Group/RFID-Tag-Guide)
- [FilamentDB](https://db.filaman.app/)
- [FilaMan](https://github.com/Fire-Devils/filaman-system)
- [Amazon Trust Services certificate repository](https://www.amazontrust.com/repository/)
- [DigiCert trusted root certificates](https://www.digicert.com/kb/digicert-root-certificates.htm)
- [Let's Encrypt certificates](https://letsencrypt.org/certificates/)
- [NXP PN532](https://www.nxp.com/products/rfid-nfc/nfc-hf/nfc-readers/standard-performance-mifare-and-ntag-frontend:PN5321A3HN)
- [Spoolman](https://github.com/Donkie/Spoolman)

## License

FilaScan retains the inherited Apache License 2.0 with Commons Clause terms.
See [`LICENSE.md`](LICENSE.md).
