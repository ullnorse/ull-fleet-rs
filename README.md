# ull-fleet-rs

A fleet server for ESP32 devices built on
[ull-esp-rs](https://github.com/ullnorse/ull-esp-rs). It runs on a PC on the home
network and serves a live web UI for phones and desktops: devices grouped by
location, their readings with charts and alert limits, firmware releases and
over-the-air updates with download progress.

- `backend/ull-fleet-server`: HTTP server, SQLite storage, JSON API with a live
  event stream; serves the built UI from inside its binary.
- `frontend/ull-fleet-ui`: the web UI, a Leptos app compiled to WebAssembly.
- `common/ull-fleet-types`: API types shared by server and UI.
- `tools/ull-fleet-sign`: offline Ed25519 signing and image inspection.
- `scripts/build-ui.sh`: builds the UI into `frontend/dist`.
- `scripts/publish.sh`: build output → signed release on the server.
- `docs/DESIGN.md`: UI scope, live updates, device protocol, update rules, security.

## Run the server

Build the UI first; the server binary embeds `frontend/dist`. It needs the
`wasm32-unknown-unknown` target, `wasm-bindgen-cli` 0.2.129 and `wasm-opt`
(binaryen).

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.129
scripts/build-ui.sh

cargo install --path backend/ull-fleet-server --root ~/.local
cargo install --path tools/ull-fleet-sign --root ~/.local

mkdir -p ~/.config/ull-fleet
cp backend/ull-fleet-server/.env.example ~/.config/ull-fleet/server.env
# set FLEET_ADMIN_PASSWORD and OTA_PUBLIC_KEY in it
ull-fleet-server --env-file ~/.config/ull-fleet/server.env
```

Open `http://<pc-ip>:3000` on the phone. Data lives in `~/.local/share/ull-fleet`
(override with `DATA_DIR`).

As a systemd user service:

```ini
# ~/.config/systemd/user/ull-fleet.service
[Service]
ExecStart=%h/.local/bin/ull-fleet-server --env-file %h/.config/ull-fleet/server.env
Restart=on-failure

[Install]
WantedBy=default.target
```

```sh
systemctl --user enable --now ull-fleet
loginctl enable-linger "$USER"   # keep it running while logged out
```

| Variable | Default | |
| --- | --- | --- |
| `LISTEN_ADDR` | `0.0.0.0:3000` | |
| `DATA_DIR` | `~/.local/share/ull-fleet` | database and release images |
| `FLEET_ADMIN_PASSWORD` | unset | without it the UI and release API are open |
| `OTA_PUBLIC_KEY` | unset | reject uploads not signed by this key |
| `POLL_INTERVAL_S` | `30` | device check-in interval |
| `LIVE_POLL_INTERVAL_S` | `5` | check-in interval while the device's page is open |
| `METRICS_RETENTION_DAYS` | `30` | older readings and events are deleted hourly |

## Devices

A device built with the `ull_esp_platform::fleet` agent checks in every 30 s
with its firmware identity, Wi-Fi signal and metrics, and appears in the UI on
its first check-in. See `examples/sht3x-ssd1306` in ull-esp-rs.

While a device's page is open the server answers its check-ins with a 5 s
interval, so readings update every few seconds; it returns to 30 s once the
page closes. Name, location and alert limits (below/above per metric) are set
on the device page. A reading outside a limit marks the device and logs an
event; returning inside clears it.

## Releases

A release is a signed ESP application image. Project and version come from the
app descriptor inside the image, which ull-esp-rs firmware sets per build
(`0.1.0+<git rev>-<build epoch>`).

```sh
# once: key pair; the secret key stays off the server
ull-fleet-sign keygen ~/.config/ull-fleet/ota-signing.key

# per release, from the ull-esp-rs workspace
cargo build --release -p sht3x-ssd1306
scripts/publish.sh target/xtensa-esp32-none-elf/release/sht3x-ssd1306 "notes"
```

Then press **Install on all** on the Releases page, or pick a release under
Firmware on one device's page. The device downloads the image at its next check-in, verifies signature, digest and
version, reboots into it, and confirms it after reaching the server. An update
that does not come back is rolled back by the device and retried up to 3 times.

`ull-fleet-sign inspect <image.bin>` prints what the server will read from an image.

## API

| | |
| --- | --- |
| `POST /api/v1/devices/{id}/checkin` | device heartbeat and metrics (JSON) |
| `GET /api/v1/devices/{id}/firmware` | target image, or 204 |
| `GET /api/v1/stream` | Server-Sent Events: a snapshot, then device, event, progress and release changes |
| `GET /api/v1/snapshot` | devices, releases, server info and recent events |
| `GET /api/v1/devices` | devices with latest readings, update state, alerts |
| `GET/PATCH/DELETE /api/v1/devices/{id}` | one device; `PATCH` takes `name`, `location`, `alert_rules` |
| `GET /api/v1/devices/{id}/watch` | event stream held open by a device page; fast check-ins while open |
| `GET /api/v1/devices/{id}/series?metric=&range=` | avg/min/max buckets; range `1h`, `6h`, `24h`, `7d`, `30d` |
| `GET /api/v1/devices/{id}/readings.csv?range=` | raw readings |
| `POST/DELETE /api/v1/devices/{id}/update` | `{"release_id": n}` / cancel |
| `GET /api/v1/events?device=&before=&limit=` | event log, newest first |
| `GET /api/v1/sparklines` | 24 h trend per device and metric |
| `GET/POST /api/v1/releases` | list / upload raw image with `X-Application-Image-Ed25519` |
| `DELETE /api/v1/releases/{id}` | a release no device runs or targets |
| `POST /api/v1/releases/{id}/deploy` | `{"devices": [...]}`, or all devices of the project |
| `GET /api/v1/session`, `POST /api/v1/login`, `POST /api/v1/logout` | login state |

All but the device endpoints need a login when `FLEET_ADMIN_PASSWORD` is set:
the session cookie or `Authorization: Bearer <password>`. Errors are
`{"error": "..."}`.

## Development

```sh
cargo test
cargo clippy --all-targets
cargo clippy -p ull-fleet-ui --target wasm32-unknown-unknown
scripts/build-ui.sh --dev   # unoptimized UI; a debug server reads frontend/dist from disk
```
