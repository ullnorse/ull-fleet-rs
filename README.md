# ull-fleet-rs

Rust workspace for fleet-management services.

## Layout

- `backend/ull-fleet-server`: backend service for OTA and fleet-management flows.
- `tools/ull-fleet-sign`: offline Ed25519 signing for OTA images.

## Backend Structure

- `src/main.rs`: loads config, builds the app, and starts Axum.
- `src/lib.rs`: composition root for wiring the OTA service.
- `src/api`: routes, handlers, request extractors, and shared Axum state.
- `src/domain`: OTA business flow and domain models.
- `src/infra`: SQLite persistence and filesystem storage.

## Backend POC

The backend now exposes a dirt-simple OTA POC:

- `POST /api/upload`
- `GET /api/update`

Environment variables:

- copy `backend/ull-fleet-server/.env.example` to `backend/ull-fleet-server/.env`
- `build.rs` loads `.env` and bakes those values into the binary for local `cargo run`
- optional `LISTEN_ADDR` (defaults to `0.0.0.0:3000`)
- `DATABASE_PATH`
- `UPLOADS_DIR`
- optional `OTA_PUBLIC_KEY`: reject uploads that are not signed by this key

Flow:

- upload one OTA image to the backend
- the ESP32 polls `GET /api/update` every 10 seconds
- if an image exists, the backend serves it once and deletes it immediately

## Signed Images

Every upload must carry an Ed25519 signature in the `X-Application-Image-Ed25519`
header. The server stores it and sends it with the image; the device verifies it
against a public key built into the firmware before activating the update. The
secret key never goes on the server.

The signed message is `"ull-esp-ota-image-v1" || sha256(image)`, matching
`ull_esp_core::signature` in ull-esp-rs. Both repos test the same vector.

```sh
# once: create a key pair, keep the secret key outside the repos
cargo run --release -p ull-fleet-sign -- keygen ~/.config/ull-fleet/ota-signing.key

# per release
SIG=$(cargo run -q --release -p ull-fleet-sign -- sign ~/.config/ull-fleet/ota-signing.key ota.bin)
curl -H "Content-Type: application/octet-stream" \
     -H "X-Application-Image-Ed25519: $SIG" \
     --data-binary @ota.bin http://127.0.0.1:3000/api/upload
```

## Development

```sh
cargo check
cargo test
```
