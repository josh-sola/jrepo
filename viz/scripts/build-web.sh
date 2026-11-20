#!/bin/sh
# Builds the viz-web wasm package into OUT_DIR/viz-web/.
#
#   viz/scripts/build-web.sh OUT_DIR
#
# Needs the wasm32-unknown-unknown Rust target and a wasm-bindgen CLI whose
# version matches the wasm-bindgen crate in Cargo.lock. Runs wasm-opt when it
# is on PATH. VIZ_BUILD_VERSION sets VERSION (default: the viz crate version).
set -eu

if [ $# -ne 1 ]; then
  echo "usage: $0 OUT_DIR" >&2
  exit 2
fi

viz_dir=$(cd "$(dirname "$0")/.." && pwd)
mkdir -p "$1"
out=$(cd "$1" && pwd)/viz-web

lock_version=$(awk '/^name = "wasm-bindgen"$/ { getline; gsub(/[^0-9.]/, ""); print; exit }' "$viz_dir/Cargo.lock")
cli_version=$(wasm-bindgen --version 2>/dev/null | awk '{ print $2 }')
if [ "$cli_version" != "$lock_version" ]; then
  echo "wasm-bindgen CLI is '${cli_version:-missing}' but Cargo.lock pins $lock_version." >&2
  echo "Install it with: cargo install wasm-bindgen-cli --version $lock_version --locked" >&2
  exit 1
fi

version=${VIZ_BUILD_VERSION:-$(awk -F'"' '/^version = / { print $2; exit }' "$viz_dir/Cargo.toml")}

cd "$viz_dir"
# Size over speed for the browser; the native viz binary keeps its profile.
CARGO_PROFILE_RELEASE_OPT_LEVEL=s CARGO_PROFILE_RELEASE_LTO=true \
  CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 CARGO_PROFILE_RELEASE_PANIC=abort \
  cargo build --release --locked -p viz-web --target wasm32-unknown-unknown

rm -rf "$out"
mkdir -p "$out/fonts"
wasm-bindgen --target web --out-dir "$out" \
  target/wasm32-unknown-unknown/release/viz_web.wasm
rm -f "$out"/*.d.ts.map "$out/viz_web_bg.wasm.d.ts"
if command -v wasm-opt >/dev/null 2>&1; then
  wasm-opt -Oz --enable-bulk-memory --enable-nontrapping-float-to-int \
    -o "$out/viz_web_bg.wasm" "$out/viz_web_bg.wasm"
fi
cp assets/fonts/Inter.ttf assets/fonts/JetBrainsMono-Regular.ttf \
  assets/fonts/JetBrainsMono-Bold.ttf "$out/fonts/"
printf '%s\n' "$version" > "$out/VERSION"

echo "Built $out ($(wc -c < "$out/viz_web_bg.wasm" | tr -d ' ') byte wasm, version $version)"
