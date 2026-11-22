#!/usr/bin/env bash
# Turns a firmware ELF into a signed release on the fleet server.
#
#   scripts/publish.sh <firmware.elf> [notes]
#
# Environment (defaults in brackets):
#   FLEET_URL             server base URL [http://127.0.0.1:3000]
#   FLEET_ADMIN_PASSWORD  admin password [read from ~/.config/ull-fleet/server.env]
#   ULL_FLEET_KEY         secret signing key [~/.config/ull-fleet/ota-signing.key]
#   PARTITION_TABLE       checks the image fits an OTA slot [none]
set -euo pipefail

elf=${1:?usage: publish.sh <firmware.elf> [notes]}
notes=${2:-}
url=${FLEET_URL:-http://127.0.0.1:3000}
key=${ULL_FLEET_KEY:-$HOME/.config/ull-fleet/ota-signing.key}
if [[ -z ${FLEET_ADMIN_PASSWORD:-} && -r $HOME/.config/ull-fleet/server.env ]]; then
    FLEET_ADMIN_PASSWORD=$(sed -n 's/^FLEET_ADMIN_PASSWORD=//p' "$HOME/.config/ull-fleet/server.env")
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
image=$work/image.bin

save=(espflash save-image --chip esp32)
if [[ -n ${PARTITION_TABLE:-} ]]; then
    save+=(--partition-table "$PARTITION_TABLE" --target-app-partition ota_0)
fi
"${save[@]}" "$elf" "$image" >/dev/null
ull-fleet-sign inspect "$image"
signature=$(ull-fleet-sign sign "$key" "$image")

auth=()
if [[ -n ${FLEET_ADMIN_PASSWORD:-} ]]; then
    auth=(-H "Authorization: Bearer $FLEET_ADMIN_PASSWORD")
fi
curl --fail-with-body -sS "${auth[@]}" \
    -H "X-Application-Image-Ed25519: $signature" \
    -H "X-Release-Notes: $notes" \
    --data-binary @"$image" \
    "$url/api/v1/releases"
echo
