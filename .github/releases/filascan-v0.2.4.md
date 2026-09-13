# FilaScan 0.2.4

FilaScan 0.2.4 is a maintenance release for offline FilaMan synchronization and
the firmware release workflow.

## Changes

- Prevent concurrent FilaMan requests from overwriting a newer offline queue
  state while completed operations are persisted.
- Separate offline inventory and queue state from FilaMan network
  orchestration.
- Use the same firmware packaging process locally and in GitHub Actions.

## Installation

Existing OTA-capable installations can update from the protected FilaScan web
interface. The merged ESP32-S3 image remains available for USB recovery.
