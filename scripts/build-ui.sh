#!/usr/bin/env bash
# Builds the Leptos web UI into frontend/dist, which the server embeds.
#   scripts/build-ui.sh          optimized build
#   scripts/build-ui.sh --dev    faster build, no wasm-opt
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
crate="$root/frontend/ull-fleet-ui"
dist="$root/frontend/dist"
profile=wasm-release
[[ "${1:-}" == "--dev" ]] && profile=dev

need() { command -v "$1" >/dev/null || { echo "missing $1: $2" >&2; exit 1; }; }
need wasm-bindgen "cargo install wasm-bindgen-cli --version 0.2.129"
[[ "$profile" == dev ]] || need wasm-opt "install binaryen"

cargo build --manifest-path "$root/Cargo.toml" -p ull-fleet-ui \
  --target wasm32-unknown-unknown --profile "$profile"
out_dir="$root/target/wasm32-unknown-unknown/$([[ $profile == dev ]] && echo debug || echo $profile)"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
wasm-bindgen --target web --no-typescript --out-dir "$work" --out-name app "$out_dir/ull-fleet-ui.wasm"
if [[ "$profile" != dev ]]; then
  wasm-opt -Oz --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext \
    --enable-mutable-globals --enable-reference-types \
    "$work/app_bg.wasm" -o "$work/app_bg.opt.wasm"
  mv "$work/app_bg.opt.wasm" "$work/app_bg.wasm"
fi

hash() { sha256sum "$1" | cut -c1-16; }
wasm_name="app-$(hash "$work/app_bg.wasm").wasm"
# The JS glue loads the wasm by the name we pass to init().
js_name="app-$(hash "$work/app.js").js"
css_name="style-$(hash "$crate/static/style.css").css"

rm -rf "$dist.tmp" && mkdir -p "$dist.tmp"
cp "$work/app_bg.wasm" "$dist.tmp/$wasm_name"
cp "$work/app.js" "$dist.tmp/$js_name"
cp "$crate/static/style.css" "$dist.tmp/$css_name"
for file in "$crate"/static/*; do
  case "$(basename "$file")" in
    style.css|index.html) ;;
    *) cp -r "$file" "$dist.tmp/" ;;
  esac
done
sed -e "s|{{css}}|/$css_name|g" -e "s|{{js}}|/$js_name|g" -e "s|{{wasm}}|/$wasm_name|g" \
  "$crate/static/index.html" > "$dist.tmp/index.html"
rm -rf "$dist" && mv "$dist.tmp" "$dist"

ls -l "$dist" | awk 'NR>1 {printf "%8d  %s\n", $5, $9}'
if command -v gzip >/dev/null; then
  echo "wasm gzipped: $(gzip -9c "$dist/$wasm_name" | wc -c) bytes"
fi
