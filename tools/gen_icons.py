#!/usr/bin/env python3
"""Generate res/mackey.ico (blue, enabled) and res/mackey_off.ico (gray).

Pure-stdlib 32x32 32bpp BMP-format ICO writer. The glyph approximates the
macOS Command symbol: a square outline with a loop at each corner.
"""
import math
import struct
import os

SIZE = 32


def dist(x, y, cx, cy):
    return math.hypot(x - cx, y - cy)


def render(bg, fg):
    px = [[None] * SIZE for _ in range(SIZE)]  # (b,g,r,a)
    # rounded-rect background
    r = 6
    for y in range(SIZE):
        for x in range(SIZE):
            inx = min(x, SIZE - 1 - x)
            iny = min(y, SIZE - 1 - y)
            if inx >= r or iny >= r or dist(inx, iny, r, r) <= r:
                px[y][x] = bg
            else:
                px[y][x] = (0, 0, 0, 0)
    # command glyph: square outline (11..20) + corner rings
    lo, hi, w = 11.0, 20.0, 1.6
    centers = [(lo - 2.2, lo - 2.2), (hi + 2.2, lo - 2.2),
               (lo - 2.2, hi + 2.2), (hi + 2.2, hi + 2.2)]
    for y in range(SIZE):
        for x in range(SIZE):
            fx, fy = x + 0.5, y + 0.5
            on = False
            # square outline
            if lo - w <= fx <= hi + w and lo - w <= fy <= hi + w:
                if not (lo + w < fx < hi - w and lo + w < fy < hi - w):
                    on = True
            # corner rings
            for cx, cy in centers:
                d = dist(fx, fy, cx, cy)
                if 2.2 <= d <= 4.4:
                    on = True
            if on and px[y][x][3] != 0:
                px[y][x] = fg
    return px


def ico_bytes(px):
    # BITMAPINFOHEADER: height doubled (XOR + AND masks), bottom-up rows
    header = struct.pack(
        "<IiiHHIIiiII", 40, SIZE, SIZE * 2, 1, 32, 0, SIZE * SIZE * 4, 0, 0, 0, 0
    )
    xor = b"".join(
        struct.pack("<BBBB", *px[y][x]) for y in range(SIZE - 1, -1, -1) for x in range(SIZE)
    )
    and_mask = b"\x00" * (SIZE * 4)  # 1bpp rows padded to 4 bytes, all opaque
    img = header + xor + and_mask
    icondir = struct.pack("<HHH", 0, 1, 1)
    entry = struct.pack("<BBBBHHII", SIZE, SIZE, 0, 0, 1, 32, len(img), 22)
    return icondir + entry + img


def main():
    out = os.path.join(os.path.dirname(__file__), "..", "res")
    os.makedirs(out, exist_ok=True)
    blue = render((235, 140, 30, 255), (255, 255, 255, 255))   # BGRA
    gray = render((130, 125, 120, 255), (210, 208, 205, 255))
    with open(os.path.join(out, "mackey.ico"), "wb") as f:
        f.write(ico_bytes(blue))
    with open(os.path.join(out, "mackey_off.ico"), "wb") as f:
        f.write(ico_bytes(gray))
    print("wrote mackey.ico, mackey_off.ico")


if __name__ == "__main__":
    main()
