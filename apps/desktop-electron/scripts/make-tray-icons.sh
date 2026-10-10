#!/bin/sh
# Rasterises the Loams favicon-cut mark (web/packages/ui/src/logo.tsx, 16-unit cut) into build/tray-{16,32}.png.
# Light body with the ochre seed reads on both light and dark panels. Run once; PNGs are committed.
set -eu
cd "$(dirname "$0")/.."
SVG='<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><g fill="#d8d4cc"><rect x="1" y="1" width="4" height="14"/><rect x="1" y="11" width="14" height="4"/><rect x="7" y="7" width="6" height="2"/></g><rect x="7" y="1" width="3" height="3" fill="#e4a24a"/></svg>'
for s in 16 32; do
	printf '%s' "$SVG" | rsvg-convert -w "$s" -h "$s" -o "build/tray-$s.png"
done
