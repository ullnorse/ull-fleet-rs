# ull-fleet-rs

Rust workspace for fleet-management services.

## Layout

- `backend/ull-fleet-server`: backend service for OTA and fleet-management flows.

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

Flow:

- upload one OTA image to the backend
- the ESP32 polls `GET /api/update` every 10 seconds
- if an image exists, the backend serves it once and deletes it immediately

Upload example:

```sh
curl -H "Content-Type: application/octet-stream" --data-binary @ota.bin http://127.0.0.1:3000/api/upload
```

## Development

```sh
cargo check
cargo test
```
