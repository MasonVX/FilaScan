# Bambu RFID Reader Fork

This branch turns the existing SpoolEase Console hardware into a focused Bambu
Lab factory-tag reader.

## Behavior

- Keeps the WT32-SC01 Plus display and PN532 wiring from upstream unchanged.
- Ignores saved printer configurations and opens no Bambu printer MQTT connection.
- Shows material, variant, color name, color code, and nominal weight immediately.
- Does not import a factory tag into the built-in inventory during scanning.
- Publishes the most recent scan at `GET /api/reader/last-scan` on the local device.

Example response:

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

The endpoint is deliberately read-only and unauthenticated on the LAN. It does
not expose Wi-Fi credentials, printer credentials, inventory records, or the
device security key.

## macOS build and flash

```bash
./scripts/bootstrap-macos.sh
./scripts/build-firmware.sh
./scripts/flash-device.sh /dev/cu.usbmodem31101
```

The first script installs host packages with Homebrew and Rust-based ESP tools
with Cargo. Native flashing is used because USB forwarding from Docker Desktop
to an ESP32 is unreliable on macOS. The Xtensa Rust fork is pinned to
`1.90.0.0`; the newest toolchains are incompatible with upstream's pinned
`esp-wifi-sys 0.8.1`.

The personal GitHub fork can be created after the one-time CLI login:

```bash
gh auth login
gh repo fork mybesttools/SpoolEase --remote --remote-name origin
git push -u origin codex/bambu-rfid-reader
```

## Spoolman bridge

The bridge only uses Python's standard library, so Docker is optional. On this
Mac it can be started without installing anything else:

```bash
READER_URL=http://192.168.5.184 \
SPOOLMAN_URL=http://YOUR-SPOOLMAN-HOST:7912 \
python3 integrations/spoolman/spoolman_bridge.py
```

For a permanently running container, copy
`integrations/spoolman/.env.example` to `.env`, enter the LAN URL of the reader,
then run:

```bash
cd integrations/spoolman
docker compose up -d --build
```

The bridge host, reader, and Spoolman host must be allowed to communicate on
the LAN. In particular, disable Wi-Fi client/AP isolation or permit traffic
between the wired and wireless segments when those devices use different
network zones.

By default the bridge creates the Bambu vendor and filament definitions only.
Set `SPOOLMAN_CREATE_SPOOL=true` to create physical spool records too. The
bridge deduplicates the two factory tags using their shared spool UID from RFID
block 9; automatic spool creation nevertheless remains opt-in so inventory
changes are always deliberate.
