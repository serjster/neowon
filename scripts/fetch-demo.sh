#!/bin/sh
# Fetch the Oscilloscope Quake stereo WAVs (lofibucket.com) for `--demo`.
set -eu
cd "$(dirname "$0")/.."
mkdir -p assets/demo
for f in e1m1_fast_48khz.wav e1m1_slow_48khz.wav; do
    if [ -f "assets/demo/$f" ]; then
        echo "assets/demo/$f already present"
    else
        echo "fetching $f…"
        curl -fSL -o "assets/demo/$f" "http://www.lofibucket.com/download/$f"
    fi
done
echo "done — run: cargo run --release -p neowon-app -- --demo"
