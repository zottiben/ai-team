#!/usr/bin/env sh
# Regenerate the icon set from mark.svg.
#
# Run from this directory:  sh regenerate.sh
#
# Needs rsvg-convert (librsvg), ImageMagick and Python with Pillow. Deliberately not
# `tauri icon`: that wants a network fetch of the CLI and produces a pile of Windows
# Store logos nothing in tauri.conf.json references.
set -eu

command -v rsvg-convert >/dev/null || { echo "need rsvg-convert (librsvg)" >&2; exit 1; }
command -v magick >/dev/null || { echo "need ImageMagick" >&2; exit 1; }

rsvg-convert -w 1024 -h 1024 mark.svg -o mark.png
cp mark.png icon.png

# PNG32: is load-bearing, not belt-and-braces. ImageMagick will happily palette-optimise
# a small flat-coloured image down to 8-bit colormap, and `tauri::generate_context!()`
# then panics at compile time with "icon ... is not RGBA" - a build failure that reads
# like a code problem and is actually this line.
magick mark.png -resize 32x32 PNG32:32x32.png
magick mark.png -resize 64x64 PNG32:64x64.png
magick mark.png -resize 128x128 PNG32:128x128.png
magick mark.png -resize 256x256 "PNG32:128x128@2x.png"
magick mark.png -define icon:auto-resize=256,128,64,48,32,16 icon.ico

# ImageMagick's ICNS writer only emits one size, which macOS then scales badly in the
# Finder. Built by hand instead: the container is a header plus length-prefixed PNGs.
python3 - <<'PY'
import struct, io
from PIL import Image

src = Image.open("mark.png").convert("RGBA")

# The PNG-based types (macOS 10.7+) only. The legacy RLE types are not worth carrying
# for an app whose minimum is 10.15.
entries = [
    (b"ic07", 128), (b"ic08", 256), (b"ic09", 512), (b"ic10", 1024),
    (b"ic11", 32),  (b"ic12", 64),  (b"ic13", 256), (b"ic14", 512),
]

chunks = []
for ostype, size in entries:
    buf = io.BytesIO()
    src.resize((size, size), Image.LANCZOS).save(buf, format="PNG")
    data = buf.getvalue()
    chunks.append(ostype + struct.pack(">I", len(data) + 8) + data)

body = b"".join(chunks)
with open("icon.icns", "wb") as f:
    f.write(b"icns" + struct.pack(">I", len(body) + 8) + body)
PY

echo "regenerated. Check 32x32.png before committing - that, not the 1024px version,"
echo "is the size it will actually be seen at."
