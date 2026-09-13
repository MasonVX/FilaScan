# FilaScan 0.2.1

This release adds PN5180 and OpenPrintTag support, improves FilaMan integration
and establishes the GitHub-hosted OTA update channel.

## Highlights

- PN532 and PN5180 reader backends with automatic reader detection
- OpenPrintTag decoding and metadata enrichment
- Bambu and OpenPrintTag product image lookup with SD-card caching
- FilaMan device registration, heartbeat, location selection and spool archival
- user-initiated firmware updates from the protected web interface
- A/B OTA installation with boot confirmation and rollback support

## Firmware assets

`FilaScan-esp32s3.bin` is the merged USB recovery and installation image for the
WT32-SC01 Plus with 16 MB flash. `FilaScan-0.2.1-ota.bin` is the application-only
OTA image. The release also contains `ota.toml`, SHA-256 checksums and build
metadata.

FilaScan is an independent community project and is not affiliated with or
endorsed by Bambu Lab, Prusa Research, OpenPrintTag or the SpoolEase maintainers.
