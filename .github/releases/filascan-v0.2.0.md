# FilaScan 0.2.0

This release extends the standalone Bambu Lab RFID reader with localized spool
data, Bambu product images and an optional FilaMan workflow.

## Highlights

- English and German display and configuration interfaces
- official color names and product codes from the BambuStudio catalog
- Bambu Store product images resolved at runtime and cached on the SD card
- configurable FilaMan integration for adding, moving and archiving spools
- location selection on the device, excluding Bambu AMS locations
- expanded live RFID diagnostics and improved PN532 retry handling

## Storage and network behavior

An SD card is recommended for persistent language and integration settings, the
BambuStudio catalog and product-image caching. Product images are looked up only
for recognized five-digit Bambu product codes. A valid cached image is reused
without a network request and does not expire automatically.

FilaMan can use HTTP on a trusted private network or HTTPS with a configured CA
certificate. The compatible FilaMan plugin must report location selection,
location management and spool archival capabilities.

## Firmware asset

`FilaScan-esp32s3.bin` is a merged image for the WT32-SC01 Plus ESP32-S3 with
16 MB flash. The release also contains its SHA-256 checksum and build metadata.

FilaScan is an independent community project and is not affiliated with or
endorsed by Bambu Lab or the SpoolEase maintainers.
