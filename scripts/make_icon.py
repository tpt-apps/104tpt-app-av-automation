#!/usr/bin/env python3
"""Generates the Windows application icon (icons/icon.ico).

The icon is a dark control-room tile with an amber cue-light dot and two fader
slots — plain shapes, no fonts, so the script needs only the standard library.
Sizes 16-256 are written as BMP-in-ICO entries, which is everything the
Windows resource compiler and the Tauri bundler need for a `.ico`.
"""

from __future__ import annotations

import struct
from pathlib import Path

BG = (24, 26, 34, 255)        # dark control-room navy
BORDER = (58, 62, 78, 255)    # subtle tile edge
AMBER = (255, 176, 32, 255)   # cue light
DIM = (96, 102, 124, 255)     # fader slots
WHITE = (232, 236, 244, 255)  # fader caps

SIZES = [16, 24, 32, 48, 64, 128, 256]


def rounded_sq(size: int, radius: int) -> list[list[bool]]:
    """A filled rounded square mask."""
    mask = [[False] * size for _ in range(size)]
    for y in range(size):
        for x in range(size):
            dx = min(x, size - 1 - x)
            dy = min(y, size - 1 - y)
            if dx + dy >= radius or (dx >= radius and dy >= radius):
                mask[y][x] = True
    return mask


def draw(size: int) -> bytes:
    """Renders one size as a 32-bit BGRA top-down BMP (BITMAPINFOHEADER, no palette)."""
    px = [[BG, BG] * 0 for _ in range(size)]
    for y in range(size):
        px[y] = [BG] * size

    mask = rounded_sq(size, max(1, size // 5))
    for y in range(size):
        for x in range(size):
            if not mask[y][x]:
                px[y][x] = (0, 0, 0, 0)

    # Tile edge: outline the mask.
    for y in range(size):
        for x in range(size):
            if mask[y][x]:
                for nx, ny in ((x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)):
                    if 0 <= nx < size and 0 <= ny < size and not mask[ny][nx]:
                        px[y][x] = BORDER
                        break

    u = size / 32.0  # design unit

    def rect(x0, y0, x1, y1, color):
        for y in range(max(0, int(y0 * u)), min(size, int(y1 * u))):
            for x in range(max(0, int(x0 * u)), min(size, int(x1 * u))):
                if mask[y][x]:
                    px[y][x] = color

    # Two fader slots.
    rect(7, 7, 9.5, 25, DIM)
    rect(15, 7, 17.5, 25, DIM)
    # Fader caps at different heights.
    rect(6, 11, 10.5, 14, WHITE)
    rect(14, 18, 18.5, 21, WHITE)
    # Cue light in the top-right corner.
    rect(23, 6, 26, 9, AMBER)

    # Encode as BGRA rows, bottom-up with inverted height (standard for 32bpp ICO BMP).
    height = size * 2  # includes the (empty) AND mask
    header = struct.pack(
        "<IiiHHIIiiII",
        40, size, height, 1, 32, 0, size * size * 4, 0, 0, 0, 0,
    )
    rows = b"".join(
        b"".join(struct.pack("<BBBB", b, g, r, a) for (r, g, b, a) in reversed(row))
        for row in reversed(px)
    )
    # The AND mask: 1bpp, opaque pixels are 0. All pixels have alpha, so all zeros.
    and_stride = ((size + 31) // 32) * 4
    and_mask = b"\x00" * (and_stride * size)
    return header + rows + and_mask


def main() -> None:
    images = []
    for size in SIZES:
        data = draw(size)
        images.append((size, data))

    out = Path(__file__).resolve().parent.parent / "crates" / "tpt-app-av-automation-tauri" / "icons" / "icon.ico"
    out.parent.mkdir(parents=True, exist_ok=True)

    ico = struct.pack("<HHH", 0, 1, len(images))
    offset = 6 + 16 * len(images)
    entries = b""
    for size, data in images:
        entries += struct.pack(
            "<BBBBHHII",
            size % 256, size % 256, 0, 0, 1, 32, len(data), offset,
        )
        offset += len(data)
    out.write_bytes(ico + entries + b"".join(d for _, d in images))
    print(f"wrote {out} ({out.stat().st_size} bytes, {len(images)} sizes)")


if __name__ == "__main__":
    main()
