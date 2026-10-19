# Write a store driver

A driver is a binary named `sepia-driver-<id>` speaking ndjson JSON-RPC
over stdio (`reference/protocol`). Implement the manifest handshake plus
the ops your store supports; drop the binary in a discovery dir
(`$SEPIA_DRIVER_DIR`, `<data-dir>/sepia/drivers`, or PATH).
