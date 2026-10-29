#!/usr/bin/env bash
# Fetch the game data — a World of Warcraft 1.12.1 (build 5875) client's Data folder — from
# archive.org. This repo ships no Blizzard content; the data is downloaded at setup time and only
# the MPQs are extracted.
#
# Re-runnable: skips when the MPQs are already present, and resumes an interrupted download.
# Requires: curl, unzip.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DATA_DIR="${WOW_DATA_DIR:-$ROOT/wow-data}"
DATA="$DATA_DIR/Data"

# --- Data source ---------------------------------------------------------------
# Where the game data is downloaded from. Override per-run by setting WOW_DATA_URL, or edit the
# default below if the archive.org item ever moves. It must be a direct link to a zip that holds
# the client's Data/*.MPQ files.
DEFAULT_DATA_URL="https://archive.org/download/WoW_1.12.1-5875/1.12.1-5875_enUS.zip"
URL="${WOW_DATA_URL:-$DEFAULT_DATA_URL}"
ZIP="$DATA_DIR/$(basename "${URL%%\?*}")"

# The two MPQs the game cannot boot without are the canary: both present = data is in.
if [ -f "$DATA/terrain.MPQ" ] && [ -f "$DATA/patch.MPQ" ]; then
    echo "game data already present at $DATA"
    echo "  --data $DATA"
    exit 0
fi

mkdir -p "$DATA_DIR"

if [ ! -f "$ZIP" ]; then
    echo "downloading the game data (a WoW 1.12.1 client, ~5.5 GB)…"
    echo "  $URL"
    curl -L -C - -o "$ZIP" "$URL"
else
    echo "resuming/using $ZIP"
fi

echo "extracting MPQs into ${DATA}…"
mkdir -p "$DATA"
unzip -o "$ZIP" 'Data/*.MPQ' -d "$DATA_DIR"

# The MPQs are what the game reads; drop the zip to reclaim ~5.5 GB.
rm -f "$ZIP"

echo "game data ready at $DATA"
echo "  --data $DATA"
