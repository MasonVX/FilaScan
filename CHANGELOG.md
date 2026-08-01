# Changelog

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
