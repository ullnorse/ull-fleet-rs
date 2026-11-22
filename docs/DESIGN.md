# ull-fleet design

A local fleet server for ESP32 devices built on ull-esp-rs. It runs on a PC on the
home network and serves a phone-first web UI.

## What the UI shows

Patterns borrowed from hosted fleet tools (balena, Golioth, Memfault), cut down to
what a handful of home devices needs:

| Page | Content |
| --- | --- |
| Devices | One row per device: status dot, name, latest readings, firmware, last seen. |
| Device | Latest readings, a chart per metric (6 h / 24 h / 7 d), device info, update status, event log. |
| Releases | Upload a signed image, list releases, deploy a release to one device or all devices of that project. |

Status follows heartbeats: a device is **online** while its last check-in is within
three poll intervals, otherwise **offline**.

## Front-end choice

Server-rendered HTML from Rust (`axum` + `maud`), a stylesheet, and ~60 lines of
plain JavaScript that re-fetches the page body every 10 s. Charts are SVG drawn on
the server.

- Everything that has logic is Rust and is type-checked; there is no node or WASM
  toolchain and the server is one binary with its assets embedded.
- Pages are a few KB and render on the first request, which matters on a phone.
- WASM frameworks (Leptos, Dioxus, Yew) were considered. They pay off for rich
  client-side interaction; this UI is mostly read-only views of server state.

## Device protocol (`/api/v1`)

Devices are identified by their Wi-Fi MAC address (12 hex digits).

`POST /api/v1/devices/{id}/checkin` with JSON:

```json
{
  "project": "sht3x-ssd1306",
  "version": "0.1.0+8775b11-1795387024",
  "secure_version": 1795387024,
  "partition": "ota_0",
  "ota_state": "valid",
  "uptime_s": 1234,
  "rssi_dbm": -61,
  "ip": "192.168.100.42",
  "last_ota_error": null,
  "metrics": { "temperature_c": 26.23, "humidity_pct": 52.5 }
}
```

Response: `{"update": true|false, "poll_interval_s": 30}`.

`GET /api/v1/devices/{id}/firmware` returns the target release image with
`Content-Length`, `X-Application-Image-Sha256` and `X-Application-Image-Ed25519`,
or `204` when the device is already on its target. The image stays on the server;
nothing is deleted when it is served.

Metric names carry their unit as a suffix (`_c`, `_pct`, `_dbm`, ...), which the UI
uses for labels and units.

## Releases and updates

- A release is a signed application image. The server reads the ESP app
  descriptor from it: project name, version string and `secure_version`.
- The version string is the release identity. Firmware builds set it to
  `<crate version>+<git rev>-<build epoch>`, and `secure_version` to the build
  epoch.
- The device rejects images whose `secure_version` is lower than its own, so a
  replayed older signed image is refused (anti-rollback without extra storage).
- Each device has an optional **target release**. The device is offered an update
  while its reported version differs from the target. Deploying sets targets.
- Attempts are tracked: if a device fetches the image and then checks in still on
  the old version (download failed, signature rejected, or bootloader rollback),
  the attempt is recorded as failed. After 3 failures the device is no longer
  offered that release until it is deployed again.

## Security

- Images are Ed25519-signed offline with `ull-fleet-sign`; the device verifies
  with a built-in public key. The server can additionally reject uploads signed
  by other keys (`OTA_PUBLIC_KEY`).
- The UI and the release/deploy API require a login when
  `FLEET_ADMIN_PASSWORD` is set. Sessions are random tokens stored in SQLite,
  sent as an `HttpOnly; SameSite=Lax` cookie (Lax so links opened from other apps keep the session; cross-site form posts are still blocked).
- Device endpoints are unauthenticated: anyone on the LAN can post readings for a
  device id. Acceptable for a home network; per-device keys are the next step.

## Storage

SQLite (`devices`, `metrics`, `events`, `releases`, `sessions`) plus a directory
of release images. Metrics older than `METRICS_RETENTION_DAYS` (default 30) are
pruned hourly. Configuration is read from the environment at startup (optionally
from a `.env` file passed with `--env-file`), not compiled in.
