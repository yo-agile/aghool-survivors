#!/usr/bin/env bash
# One command from a fresh clone to a playable browser build:
#
#   ./scripts/setup.sh
#
#   1. fetch the game data (a WoW 1.12.1 client's Data folder, from archive.org — not committed)
#   2. install the wasm toolchain (wasi-sdk, wasm-bindgen-cli, binaryen)
#   3. build the client into web/dist/
#
# Each step is re-runnable and skips what is already done.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

scripts/fetch-data.sh
scripts/web-setup.sh
scripts/web-build.sh

echo
echo "All set. Start the local server:"
echo "  cargo run --release -p wenilla-host -- --www web/dist --data wow-data/Data"
echo "then open http://127.0.0.1:8090/"
