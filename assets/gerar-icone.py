"""Gera assets/painel-led.ico com o mesmo desenho do ícone da bandeja (src/main.rs, icon_rgba)."""
import math
from pathlib import Path
from PIL import Image

LIT = [[1, 1, 1, 1], [1, 0, 0, 1], [1, 0, 0, 1], [1, 1, 1, 1]]


def icon(size: int) -> Image.Image:
    s = float(size)
    img = Image.new("RGBA", (size, size))
    px = img.load()
    for y in range(size):
        for x in range(size):
            fx, fy = x + 0.5, y + 0.5
            r = s * 0.22
            cx, cy = min(max(fx, r), s - r), min(max(fy, r), s - r)
            bg_a = min(max(r - math.hypot(fx - cx, fy - cy) + 0.5, 0.0), 1.0)
            cell = s / 4.0
            gx, gy = min(int(fx / cell), 3), min(int(fy / cell), 3)
            lx, ly = gx * cell + cell / 2, gy * cell + cell / 2
            dot = min(max(cell * 0.36 - math.hypot(fx - lx, fy - ly) + 0.5, 0.0), 1.0)
            led = (255, 176, 0) if LIT[gy][gx] else (60, 60, 60)
            c = [20 * (1 - dot) + led[i] * dot for i in range(3)]
            a = max(bg_a, dot * bg_a)
            px[x, y] = (int(c[0]), int(c[1]), int(c[2]), int(a * 255))
    return img


out = Path(__file__).with_name("painel-led.ico")
sizes = [16, 24, 32, 48, 64, 128, 256]
icon(256).save(out, sizes=[(n, n) for n in sizes], append_images=[icon(n) for n in sizes[:-1]])
print(out)
