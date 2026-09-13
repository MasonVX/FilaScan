# FilaScan release publication

The workflow triggers on tags matching `filascan-v*`. Its tag validation requires the numeric tag suffix to equal the version in `core/Cargo.toml` and requires matching release notes at `.github/releases/<tag>.md`.

The shared packaging script produces:

- `FilaScan-esp32s3.bin`: merged USB recovery image
- `FilaScan-<version>-ota.bin`: application-only OTA image
- `ota.toml`: OTA filename, version, size, and CRC32
- `SHA256SUMS`: checksums for both binaries
- `build-info.txt`: project, version, commit, target, and flash size

After pushing the annotated tag, locate the tag-triggered `firmware.yml` run and wait for all three jobs:

1. ESP32-S3 build/package
2. GitHub Release publication
3. GitHub Pages OTA deployment

Verify that the release contains every expected artifact. Verify the Pages `ota.toml` only after deployment completes; CDN propagation can briefly expose the previous version. Keep the merged binary available for USB recovery if OTA installation fails.
