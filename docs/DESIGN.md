# ull-fleet design

A local fleet server for ESP32 devices built on ull-esp-rs. It runs on a PC on the
home network and serves a live web UI, designed for touch on phones and for mouse
and keyboard on desktops.

## What the UI shows

Patterns borrowed from hosted fleet tools (balena, Golioth, Memfault), cut down to
what a handful of home devices needs:

| Page | Content |
| --- | --- |
| Devices | Online / alert / updating counts; a card per device grouped by location: status, signal, two readings with 24 h sparklines, update progress or last seen and version. |
| Device | Status with a check-in countdown; a chart per metric (1 h to 30 d, average line, min/max band, alert limits, crosshair); firmware with an update stepper and download progress; alert limits; details; the device's events; CSV export. |
| Trends | One metric across every device that reports it: a line per device in a fixed color, a crosshair listing all values, a legend that hides lines. |
| Activity | All events grouped by day, filtered by alerts, updates or connectivity. |
| Releases | Per project: releases with rollout coverage, install on all, delete unused; upload by drag and drop. |
| Settings | Theme, check-in intervals, login state, signing key fingerprint, server version. |

Status follows heartbeats: a device is **online** while its last check-in is within
three of its poll intervals (at least 60 s), otherwise **offline**. A device with
a reading outside an alert limit shows as **alert**.

## Live updates

- `GET /api/v1/stream` is a Server-Sent Events stream. It starts with a snapshot
  (devices, releases, recent events, server info) and then sends each change:
  a device after its check-in, a new event, download progress, the release list.
  The UI keeps one stream open and applies the changes to its store; on a
  reconnect the new snapshot replaces the store, so nothing is missed.
- A device page also opens `GET /api/v1/devices/{id}/watch`. While at least one
  is open, that device's check-in replies carry `LIVE_POLL_INTERVAL_S` (5 s)
  instead of `POLL_INTERVAL_S`; the device switches at its next check-in. The
  offline threshold follows the interval the device was last told.
- Download progress counts the bytes of the image handed to the socket. The
  listener's send buffer is 32 KB, so the count stays within about that much of
  what the device has received.
- Offline transitions are found by a 2 s timer, since no request marks them.

## Front-end choice

Leptos 0.8, client-side rendered, compiled to WebAssembly. The first version was
server-rendered HTML (`maud`) refreshed every 10 s; live updates, crosshair
charts, inline editing and upload progress made a client-side app the better fit.

- API types live in `ull-fleet-types` and are used by both sides, so a changed
  field is a compile error in the UI rather than a runtime surprise.
- Client-side rendering rather than SSR with hydration: on a LAN the cached
  bundle (about 410 KB compressed, hashed file names, cached forever) loads
  fast, and SSR would compile every component twice and risk hydration
  mismatches with data that changes every few seconds.
- Charts are SVG built in Rust, drawn with a fixed viewBox and
  `vector-effect: non-scaling-stroke`; labels, crosshair and tooltip are HTML
  placed in percent, so text never stretches.
- The server embeds `frontend/dist` with `rust-embed` and serves `index.html`
  for unknown non-API paths, so client routes work on reload.

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

Response: `{"update": true|false, "poll_interval_s": 30}`. The device uses the
returned interval for its next check-in (5 s while its page is open).

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

SQLite (`devices`, `metrics`, `latest_metrics`, `events`, `releases`,
`sessions`, `alert_rules`, `active_alerts`) plus a directory of release images. Metrics older than `METRICS_RETENTION_DAYS` (default 30) are
pruned hourly. Configuration is read from the environment at startup (optionally
from a `.env` file passed with `--env-file`), not compiled in.
