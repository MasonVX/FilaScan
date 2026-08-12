# Changelog

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
