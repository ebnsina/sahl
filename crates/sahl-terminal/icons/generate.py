"""Generate the placeholder app icons Tauri requires at compile time.

A solid square in the brand indigo (--color-primary, oklch(0.51 0.235 277) ≈ #4F46E5). Deliberately
plain: a real icon is a design task, and a wrong-but-pretty placeholder is harder to notice and
replace than an obviously blank one.
"""

import pathlib
import struct
import zlib


def png(size: int, rgba: tuple[int, int, int, int]) -> bytes:
    # One filter byte (0 = None) per scanline, then RGBA quadruples.
    raw = b"".join(b"\x00" + bytes(rgba) * size for _ in range(size))

    def chunk(tag: bytes, data: bytes) -> bytes:
        body = tag + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)

    # Colour type 6 is RGBA. Tauri rejects plain RGB (type 2) outright.
    header = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", header)
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


INDIGO = (79, 70, 229, 255)
icons = pathlib.Path("crates/sahl-terminal/icons")
icons.mkdir(exist_ok=True)

for name, size in [("icon.png", 512), ("32x32.png", 32), ("128x128.png", 128), ("128x128@2x.png", 256)]:
    data = png(size, INDIGO)
    (icons / name).write_bytes(data)
    print(f"  {name:<18} {size}x{size}  {len(data)} bytes")

assert (icons / "icon.png").read_bytes()[:8] == b"\x89PNG\r\n\x1a\n", "PNG signature must be valid"
print("PNG signature verified")
