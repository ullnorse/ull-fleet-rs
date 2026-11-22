# ull-fleet-rs

A fleet server for ESP32 devices built on
[ull-esp-rs](https://github.com/ullnorse/ull-esp-rs). It runs on a PC on the home
network and serves a web UI for phones and desktops: devices, their readings with charts, firmware
releases and over-the-air updates.

- `backend/ull-fleet-server`: HTTP server, SQLite storage, web UI.
- `tools/ull-fleet-sign`: offline Ed25519 signing and image inspection.
- `scripts/publish.sh`: build output → signed release on the server.
- `docs/DESIGN.md`: UI scope, device protocol, update rules, security.

## Run the server

```sh
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
| `METRICS_RETENTION_DAYS` | `30` | older readings and events are deleted hourly |

## Devices

A device built with the `ull_esp_platform::fleet` agent checks in every 30 s
with its firmware identity, Wi-Fi signal and metrics, and appears in the UI on
its first check-in. See `examples/sht3x-ssd1306` in ull-esp-rs.

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

Then press **Update all** on the Releases page, or **Update** on one device. The
device downloads the image at its next check-in, verifies signature, digest and
version, reboots into it, and confirms it after reaching the server. An update
that does not come back is rolled back by the device and retried up to 3 times.

`ull-fleet-sign inspect <image.bin>` prints what the server will read from an image.

## API

| | |
| --- | --- |
| `POST /api/v1/devices/{id}/checkin` | device heartbeat and metrics (JSON) |
| `GET /api/v1/devices/{id}/firmware` | target image, or 204 |
| `GET /api/v1/devices` | device summaries (admin) |
| `GET/POST /api/v1/releases` | list / upload raw image with `X-Application-Image-Ed25519` (admin) |
| `POST /api/v1/releases/{id}/deploy` | `{"devices": [...]}`, or all devices of the project (admin) |

Admin calls take the session cookie or `Authorization: Bearer <password>`.

## Development

```sh
cargo test
cargo clippy --all-targets
```
