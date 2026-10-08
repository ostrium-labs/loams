#!/bin/sh
# Rasterises the Loams mark (full cut of web/packages/ui/src/logo.tsx, same colours as the tray icons)
# on a dark rounded tile into build/icon.png (512), build/icon-256.png, build/icon.ico and build/icon.icns.
# Needs rsvg-convert and ImageMagick. Run when the logo changes; outputs are committed.
set -eu
cd "$(dirname "$0")/.."
SVG='<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><rect width="64" height="64" rx="12" fill="#1b1a17"/><g transform="translate(12 12) scale(1.5385)"><g transform="translate(-3 -3)"><g fill="#d8d4cc"><rect x="3" y="3" width="6" height="26"/><rect x="3" y="23" width="26" height="6"/><rect x="12" y="16" width="14" height="4"/><rect x="12" y="10" width="9" height="3"/></g><rect x="12" y="3" width="4" height="4" fill="#e4a24a"/></g></g></svg>'
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
for s in 16 32 48 64 128 256 512 1024; do
	printf '%s' "$SVG" | rsvg-convert -w "$s" -h "$s" -o "$tmp/$s.png"
done
cp "$tmp/512.png" build/icon.png
cp "$tmp/256.png" build/icon-256.png
magick "$tmp/16.png" "$tmp/32.png" "$tmp/48.png" "$tmp/64.png" "$tmp/128.png" "$tmp/256.png" build/icon.ico
# icns is a container of PNGs: ic07=128, ic08=256, ic09=512, ic10=1024.
node -e '
const fs = require("fs");
const [dir, out] = process.argv.slice(1);
const parts = [["ic07",128],["ic08",256],["ic09",512],["ic10",1024]].map(([t, s]) => {
	const png = fs.readFileSync(`${dir}/${s}.png`);
	const h = Buffer.alloc(8); h.write(t, 0, "ascii"); h.writeUInt32BE(png.length + 8, 4);
	return Buffer.concat([h, png]);
});
const body = Buffer.concat(parts);
const head = Buffer.alloc(8); head.write("icns", 0, "ascii"); head.writeUInt32BE(body.length + 8, 4);
fs.writeFileSync(out, Buffer.concat([head, body]));
' "$tmp" build/icon.icns
