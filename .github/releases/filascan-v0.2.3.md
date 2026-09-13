# FilaScan 0.2.3

FilaScan 0.2.3 adds offline FilaMan storage and fixes the OTA installation
panic found while testing 0.2.2.

## Changes

- Cache the active FilaMan spool and regular-location inventory in RAM.
- Persist the deterministic inventory snapshot to the SD card only when its
  contents change.
- Allow a scanned spool to be assigned to a cached location without Wi-Fi or a
  reachable FilaMan server.
- Keep one pending target location per spool and synchronize it through the
  existing FilaScan plugin endpoints after connectivity recovers.
- Exclude the physical NFC Tag UID from persisted offline operations.
- Show FilaMan connectivity, cache size and pending-operation count in the
  protected web interface.
- Fix the nested framework borrow that could panic the device when an OTA
  installation started.

## Installation

Existing OTA-capable installations can update from the protected FilaScan web
interface. The merged ESP32-S3 image remains available for USB recovery or a
first installation. Offline FilaMan operations require a FAT-formatted microSD
card and a location inventory downloaded during an earlier online connection.
