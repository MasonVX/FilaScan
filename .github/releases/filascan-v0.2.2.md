# FilaScan 0.2.2

This release adds an on-device firmware status page as the first update
distributed through FilaScan's GitHub-hosted OTA channel.

## Highlights

- swipe upward from the bottom edge to open the firmware page
- display the installed firmware version
- check the GitHub OTA channel whenever the page is opened
- show current, available and failed update states in English or German
- return with the Back button or a downward swipe from the top edge

Firmware installation remains an explicit, confirmed action in the protected
web interface.

## Firmware assets

`FilaScan-esp32s3.bin` is the merged USB recovery image.
`FilaScan-0.2.2-ota.bin` is the application-only OTA image used by the device.
The release also contains the OTA manifest, SHA-256 checksums and build metadata.
