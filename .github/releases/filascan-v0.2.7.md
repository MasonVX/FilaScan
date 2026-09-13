# FilaScan 0.2.7

- Connect to FilaMan as soon as Wi-Fi is ready, without the previous startup delay.
- Wait for the first connection check when scanning during startup.
- Automatically retry queued offline operations while connected.
- Prevent concurrent queue writes and a stuck busy state after SD write failures.
- Display a yellow outlined Offline indicator beside the spool status.

## Installation

Install via the firmware page on the device or the protected web interface.
For USB installation or recovery, use https://masonvx.github.io/FilaScan/.
Leave Erase device unchecked to keep existing FilaScan settings with a compatible
partition layout. Both USB modes leave SD-card contents and FilaMan tokens intact.

The firmware build is validated locally; the updated connection behavior and
display layout still require verification on hardware.
