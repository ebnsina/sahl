"""Generate the app icons from the Sahl mark.

The mark is a receipt reduced to three strokes — two full bars and a short one — on the brand
indigo. Drawn here rather than exported from a design tool so the icon cannot drift from
`packages/ui/src/components/Logo.svelte`: both are the same four rectangles on a 32-unit grid.

Written by hand rather than with an image library on purpose. This runs on a fresh machine and in
CI, and a build step that needs Pillow installed is a build step that breaks on someone else's
laptop.
"""

import pathlib
import struct
import zlib

# oklch(0.51 0.235 277), the --color-primary token, in sRGB.
INDIGO = (79, 70, 229, 255)
WHITE = (255, 255, 255, 255)

# The mark on a 32-unit grid: (x, y, width, height), matching Logo.svelte exactly.
BARS = [
    (7, 9, 18, 3),
    (7, 15, 18, 3),
    (7, 21, 9, 3),
]


def render(size: int) -> bytes:
    """The mark at `size` px.

    Nearest-neighbour from the 32-unit grid, which keeps every edge crisp. The sizes below are all
    whole multiples of 32, so no bar ever lands on a half pixel and nothing needs anti-aliasing.
    """
    scale = size / 32
    rows = []
    for y in range(size):
        row = bytearray(b"\x00")  # filter byte: None
        unit_y = y / scale
        for x in range(size):
            unit_x = x / scale
            pixel = INDIGO
            for bar_x, bar_y, bar_w, bar_h in BARS:
                if bar_x <= unit_x < bar_x + bar_w and bar_y <= unit_y < bar_y + bar_h:
                    pixel = WHITE
                    break
            row += bytes(pixel)
        rows.append(bytes(row))
    return b"".join(rows)


def png(size: int) -> bytes:
    def chunk(tag: bytes, data: bytes) -> bytes:
        body = tag + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)

    # Colour type 6 is RGBA. Tauri rejects plain RGB (type 2) outright.
    header = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", header)
        + chunk(b"IDAT", zlib.compress(render(size), 9))
        + chunk(b"IEND", b"")
    )


icons = pathlib.Path("crates/sahl-terminal/icons")
icons.mkdir(exist_ok=True)

for name, size in [
    ("icon.png", 512),
    ("32x32.png", 32),
    ("128x128.png", 128),
    ("128x128@2x.png", 256),
]:
    data = png(size)
    (icons / name).write_bytes(data)
    print(f"  {name:<18} {size}x{size}  {len(data)} bytes")

signature = (icons / "icon.png").read_bytes()
assert signature[:8] == b"\x89PNG\r\n\x1a\n", "PNG signature must be valid"
assert signature[25] == 6, "colour type must be 6 (RGBA) — Tauri rejects type 2"
print("PNG signature and RGBA colour type verified")
