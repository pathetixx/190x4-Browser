#!/usr/bin/env python3
"""Иконка приложения 190x4 Browser: они-маска 190x4 в круглом знаке.

Круг — форма, по которой браузер узнают на панели задач (Chrome, Edge,
Firefox, Opera). Маска вырывается пламенем за обод: знак остаётся знаком 190x4,
а не очередным кругом. Тёмный диск с багровым ободом держит контраст и на
тёмной, и на светлой панели задач.

Мелкие размеры собираются отдельно, а не уменьшаются из 1024: обод толщиной
в 26 px на мастере превратился бы на 16 px в полупиксельную муть.

    python3 scripts/icons/build_app_icon.py
"""
import io
import math
import struct
from pathlib import Path

from PIL import Image, ImageChops, ImageDraw, ImageFilter

ROOT = Path(__file__).resolve().parents[2]
ICONS = ROOT / "src-tauri" / "icons"
BRAND = ROOT / "ui" / "assets" / "brand"
S = 1024

DISC_IN, DISC_OUT = (42, 16, 26), (9, 8, 12)
RING_TOP, RING_BOTTOM = (232, 86, 111), (112, 26, 42)


def radial(size: int) -> Image.Image:
    small = 256
    img = Image.new("RGB", (small, small))
    px = img.load()
    for y in range(small):
        for x in range(small):
            t = min(1.0, math.hypot(x / small - 0.5, y / small - 0.42) / 0.62)
            px[x, y] = tuple(int(DISC_IN[i] + (DISC_OUT[i] - DISC_IN[i]) * t) for i in range(3))
    return img.resize((size, size), Image.BICUBIC).convert("RGBA")


def vertical(size: int) -> Image.Image:
    column = Image.new("RGB", (1, size))
    for y in range(size):
        t = y / (size - 1)
        column.putpixel((0, y), tuple(int(RING_TOP[i] + (RING_BOTTOM[i] - RING_TOP[i]) * t) for i in range(3)))
    return column.resize((size, size)).convert("RGBA")


def disc(pad: float) -> Image.Image:
    mask = Image.new("L", (S * 2, S * 2), 0)
    ImageDraw.Draw(mask).ellipse((pad * 2, pad * 2, (S - pad) * 2, (S - pad) * 2), fill=255)
    return mask.resize((S, S), Image.LANCZOS)


def master(target: int) -> Image.Image:
    art = Image.open(ICONS / "source" / "oni.png").convert("RGBA")
    art = art.crop(art.getbbox())

    # Обод не тоньше ~1.3 px итогового размера.
    ring = max(26.0, S / target * 1.3)
    small = target <= 32
    outer = 60 if small else 92
    scale = 900 if small else 860

    canvas = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    canvas.paste(vertical(S), mask=disc(outer))
    canvas.paste(radial(S), mask=disc(outer + ring))

    k = min(scale / art.width, scale / art.height)
    art = art.resize((int(art.width * k), int(art.height * k)), Image.LANCZOS)
    layer = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    layer.alpha_composite(art, ((S - art.width) // 2, (S - art.height) // 2 - 6))

    # Внутри диска маска видна целиком, над экватором — может выходить за обод.
    allowed = disc(outer + ring)
    top = Image.new("L", (S, S), 0)
    ImageDraw.Draw(top).rectangle((0, 0, S, int(S * 0.52)), fill=255)
    allowed = ImageChops.lighter(allowed, top)
    layer.putalpha(ImageChops.multiply(layer.getchannel("A"), allowed))
    canvas.alpha_composite(layer)
    return canvas


def render(size: int) -> Image.Image:
    image = master(size).resize((size, size), Image.LANCZOS)
    if size <= 48:
        image = image.filter(ImageFilter.UnsharpMask(radius=0.6, percent=70, threshold=1))
    return image


def write_ico(path: Path, sizes: list[int]) -> None:
    """ICO с PNG внутри на каждый размер: каждый кадр отрисован отдельно."""
    blobs = []
    for size in sizes:
        buffer = io.BytesIO()
        render(size).save(buffer, format="PNG", optimize=True)
        blobs.append((size, buffer.getvalue()))

    header = struct.pack("<HHH", 0, 1, len(blobs))
    offset = 6 + 16 * len(blobs)
    entries, data = b"", b""
    for size, blob in blobs:
        side = 0 if size >= 256 else size
        entries += struct.pack("<BBBBHHII", side, side, 0, 0, 1, 32, len(blob), offset + len(data))
        data += blob
    path.write_bytes(header + entries + data)


def main() -> None:
    BRAND.mkdir(parents=True, exist_ok=True)
    render(512).save(ICONS / "icon.png", optimize=True)
    render(256).save(ICONS / "128x128@2x.png", optimize=True)
    render(128).save(ICONS / "128x128.png", optimize=True)
    render(32).save(ICONS / "32x32.png", optimize=True)
    write_ico(ICONS / "icon.ico", [16, 20, 24, 32, 40, 48, 64, 128, 256])
    # Знак в строке заголовка: 18 CSS-пикселей, до 400% масштаба Windows.
    render(72).save(BRAND / "mark-72.png", optimize=True)
    print("иконки собраны")


if __name__ == "__main__":
    main()
