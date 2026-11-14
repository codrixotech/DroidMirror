#!/usr/bin/env python3
"""Renders packaging/icon-1024.png (the app icon) without image libraries.

A dark rounded square (macOS icon grid: 824 pt body in a 1024 canvas) with
a white phone outline and a green screen. Anti-aliased by 4x4 supersampling.
Run: python3 scripts/make-icon.py packaging/icon-1024.png
"""
import struct
import sys
import zlib

SIZE = 1024
SS = 4  # supersampling per axis


def rounded_rect(x, y, x0, y0, x1, y1, r):
    """Inside test for a rounded rectangle."""
    if x < x0 or x > x1 or y < y0 or y > y1:
        return False
    cx = min(max(x, x0 + r), x1 - r)
    cy = min(max(y, y0 + r), y1 - r)
    return (x - cx) ** 2 + (y - cy) ** 2 <= r * r


BG = (0x11, 0x18, 0x27)
PHONE = (0xF9, 0xFA, 0xFB)
SCREEN = (0x34, 0xD3, 0x99)
NOTCH = (0x11, 0x18, 0x27)


def sample(x, y):
    """Colour (or None for transparent) at canvas point x, y."""
    if not rounded_rect(x, y, 100, 100, 924, 924, 185):
        return None
    if rounded_rect(x, y, 437, 255, 587, 285, 15):  # speaker slot
        return NOTCH
    if rounded_rect(x, y, 372, 300, 652, 724, 18):
        return SCREEN
    if rounded_rect(x, y, 337, 215, 687, 809, 62):
        return PHONE
    return BG


def render():
    rows = []
    step = 1.0 / SS
    for py in range(SIZE):
        row = bytearray([0])  # filter type 0
        for px in range(SIZE):
            r = g = b = a = 0
            for sy in range(SS):
                for sx in range(SS):
                    c = sample(px + (sx + 0.5) * step, py + (sy + 0.5) * step)
                    if c:
                        r += c[0]
                        g += c[1]
                        b += c[2]
                        a += 1
            n = SS * SS
            if a:
                row += bytes((r // a, g // a, b // a, 255 * a // n))
            else:
                row += b"\0\0\0\0"
        rows.append(bytes(row))
    return b"".join(rows)


def png(raw):
    def chunk(kind, data):
        c = struct.pack(">I", len(data)) + kind + data
        return c + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)

    header = struct.pack(">IIBBBBB", SIZE, SIZE, 8, 6, 0, 0, 0)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header)
            + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))


if __name__ == "__main__":
    out = sys.argv[1] if len(sys.argv) > 1 else "packaging/icon-1024.png"
    with open(out, "wb") as f:
        f.write(png(render()))
    print("wrote", out)
