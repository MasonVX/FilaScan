# Changelog

## Unreleased

- Added an on-device firmware page opened by swiping upward from the bottom
  edge. It shows the installed version and checks the GitHub OTA channel whenever
  the page is opened; installation remains in the protected web interface.
- Added user-initiated A/B OTA updates, GitHub Pages publication, boot
  confirmation and rollback support.

- Reduced PN5180 tag-removal confirmation to 750 ms and transient RF recovery
  to two seconds. Payload reads now stop after two consecutive attempts without
  additional blocks while retaining the five-attempt maximum.
- Added a boot-only Wi-Fi fallback that starts the setup access point after 60
  seconds without a connection or IPv4 address while retaining stored
  credentials. Later connection losses do not activate the fallback.
- Added the official FilaMan device heartbeat with local IPv4 reporting so
  FilaMan can display the registered device's online state and address.
- Added the ESP32 reset reason to the startup diagnostic log for passive crash
  diagnosis without a persistent USB or browser connection.
- Added one-time FilaMan device-code registration to the protected web
  interface. FilaScan exchanges the six-character code for a permanent device
  token, validates it and stores it on the SD card without logging the code or
  token. The stored token is never returned to the browser; the UI exposes only
  registration status, non-secret device identity and local logout.
- Serialized background log, catalog and FilaMan status polling in the web
  interface so browser keep-alive requests do not exhaust the device's two HTTP
  workers while the protected configuration is being unlocked.
- Added separate PN532 and PN5180 reader backends behind a common event
  interface.
- Added automatic reader detection with PN5180 version validation and PN532
  fallback.
- Added PN5180 ISO/IEC 14443-A activation, hardware MIFARE Classic
  authentication and Bambu payload reading.
- Added PN5180 receive-status validation, tag-removal debouncing and targeted
  hardware recovery for BUSY, SPI and pin failures.
- Kept completed PN5180 scans latched until the RFID field has been continuously
  empty, preventing repeated reads of a stationary spool and alternation between
  its two tags.
- Added build-time reader overrides through `FILASCAN_RFID_READER`.

## 0.2.0 - 2026-08-12

- Added English and German device and configuration interfaces with persistent
  language selection.
- Added validated BambuStudio catalog downloads, localized official color
  names, daily updates and an SD-card catalog cache.
- Added Bambu EU Store product-image resolution without a bundled static map.
- Added validated 240-pixel JPEG downloads and a persistent SD-card image cache.
- Added configurable FilaMan plugin integration using the Bambu Tray UID as the
  external spool identifier.
- Added user-confirmed location selection for new spools while excluding Bambu
  AMS locations.
- Added location changes and confirmed archival for registered FilaMan spools.
- Added configurable HTTP or certificate-validated HTTPS connections to
  FilaMan.
- Expanded the live diagnostic console with decoded tag fields and raw payload
  blocks while keeping authentication keys and credentials out of the log.
- Improved PN532 retry and target-reacquisition behavior for intermittent RFID
  reads.
- Added tag-driven GitHub releases for the merged ESP32-S3 image, checksum and
  build metadata.

## 0.1.0

- Replaced the SpoolEase application layer with a standalone Bambu RFID reader.
- Added direct display of material, official color mapping and spool parameters.
- Added a large on-device color preview with two-color tag support.
- Limited the web interface to Wi-Fi provisioning.
- Added a continuously updated web diagnostic log for RFID reads and retries.
- Separated unsupported tag types from transient authentication/read failures.
- Added PN532 passive-target reacquisition and partial-read reuse for weakly
  coupled tags.
- Removed inventory, printer/AMS MQTT, scale, tag-writing, print-analysis and
  Spoolman bridge code.
- Renamed the firmware package and build output to FilaScan.
