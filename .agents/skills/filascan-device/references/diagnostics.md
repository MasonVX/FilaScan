# FilaScan diagnostics

## Safe observation order

1. Check whether the display, touch input, and web interface respond.
2. Capture the bounded live diagnostics from the web interface if available.
3. Identify the USB port with `scripts/detect-device-port.sh`.
4. Attach with `scripts/monitor-device.sh`; this uses `espflash monitor --non-interactive --no-reset`.
5. Reproduce one event at a time and correlate reader messages, reset reason, Wi-Fi state, and elapsed timestamps.
6. Reset or reflash only after the current failure evidence has been captured.

## Interpret separately

- A web request failure alone does not prove a firmware hang.
- Simultaneous loss of display/touch and web service is stronger evidence of a device-wide stall or reset.
- Reader authentication, receive-status, and timeout errors can be RF/SPI recovery failures without implying a whole-device crash.
- Opening a serial session with default `espflash monitor` behavior resets the target and can erase the state being investigated.

Record the exact firmware version/commit, board, reader backend, port, reset reason, and the last log lines when reporting a diagnosis.
