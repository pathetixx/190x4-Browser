#!/usr/bin/env python3
"""Иконка приложения 190x4 Browser: «глобус-маска».

Багровая сфера с меридианами — форма, по которой браузер узнают на панели
задач, — и тёмный силуэт они-маски 190x4 поверх неё. От иконки Ninety знак
отличается и формой, и цветом: там маска в полном цвете.

Векторные части рисует `rsvg-convert` сразу в нужном размере, силуэт маски —
альфа-канал `icons/source/oni.png`. До 32 px включительно — упрощённый
вариант: меридианы сливаются в муть, остаётся экватор потолще, маска крупнее.

    python3 scripts/icons/build_app_icon.py     # нужен rsvg-convert (librsvg)
"""
import base64
import io
import struct
import subprocess
import tempfile
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parents[2]
ICONS = ROOT / "src-tauri" / "icons"
BRAND = ROOT / "ui" / "assets" / "brand"
SMALL = 32


def silhouette() -> str:
    art = Image.open(ICONS / "source" / "oni.png").convert("RGBA")
    art = art.crop(art.getbbox())
    side = max(art.size)
    square = Image.new("RGBA", (side, side), (0, 0, 0, 0))
    square.paste(art, ((side - art.width) // 2, (side - art.height) // 2))
    alpha = square.resize((1024, 1024), Image.LANCZOS).split()[3]
    mask = Image.new("RGBA", (1024, 1024), (12, 9, 13, 255))
    mask.putalpha(alpha)
    buffer = io.BytesIO()
    mask.save(buffer, "PNG")
    return "data:image/png;base64," + base64.b64encode(buffer.getvalue()).decode()


def svg(mask: str, small: bool) -> str:
    size = 372 if small else 334
    x = (512 - size) / 2
    y = x + (6 if small else 12)
    if small:
        meridians = '<ellipse cx="256" cy="256" rx="232" ry="92" stroke-width="16"/>'
        shade = ""
    else:
        meridians = (
            '<ellipse cx="256" cy="256" rx="232" ry="92" stroke-width="7"/>'
            '<ellipse cx="256" cy="256" rx="96" ry="232" stroke-width="7"/>'
        )
        shade = (
            '<circle cx="256" cy="256" r="232" fill="url(#shade)"/>'
            '<circle cx="256" cy="256" r="230.5" fill="none" stroke="#3a0a17" '
            'stroke-opacity=".55" stroke-width="3"/>'
        )
    opacity = 0.30 if small else 0.24
    return f"""<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" viewBox="0 0 512 512">
<defs>
<radialGradient id="globe" cx="36%" cy="30%" r="78%"><stop offset="0" stop-color="#ff7087"/><stop offset=".42" stop-color="#d7395a"/><stop offset=".78" stop-color="#9c1f37"/><stop offset="1" stop-color="#5a0d1e"/></radialGradient>
<radialGradient id="shade" cx="50%" cy="50%" r="50%"><stop offset=".74" stop-color="#000" stop-opacity="0"/><stop offset="1" stop-color="#000" stop-opacity=".32"/></radialGradient>
<clipPath id="sphere"><circle cx="256" cy="256" r="232"/></clipPath>
</defs>
<circle cx="256" cy="256" r="232" fill="url(#globe)"/>
<g clip-path="url(#sphere)" fill="none" stroke="#fff" stroke-opacity="{opacity}">{meridians}</g>
<image x="{x}" y="{y}" width="{size}" height="{size}" href="{mask}" xlink:href="{mask}"/>
{shade}
</svg>"""


class Renderer:
    def __init__(self) -> None:
        mask = silhouette()
        self.dir = tempfile.TemporaryDirectory()
        self.full = Path(self.dir.name) / "full.svg"
        self.small = Path(self.dir.name) / "small.svg"
        self.full.write_text(svg(mask, small=False))
        self.small.write_text(svg(mask, small=True))

    def render(self, size: int) -> Image.Image:
        source = self.small if size <= SMALL else self.full
        out = Path(self.dir.name) / f"{size}.png"
        subprocess.run(
            ["rsvg-convert", "-w", str(size), "-h", str(size), "-o", str(out), str(source)],
            check=True,
        )
        return Image.open(out).convert("RGBA")


def write_ico(renderer: Renderer, path: Path, sizes: list[int]) -> None:
    images = []
    for size in sizes:
        buffer = io.BytesIO()
        renderer.render(size).save(buffer, format="PNG", optimize=True)
        images.append((size, buffer.getvalue()))
    header = struct.pack("<HHH", 0, 1, len(images))
    offset = 6 + 16 * len(images)
    entries = b""
    for size, data in images:
        edge = 0 if size >= 256 else size
        entries += struct.pack("<BBBBHHII", edge, edge, 0, 0, 1, 32, len(data), offset)
        offset += len(data)
    path.write_bytes(header + entries + b"".join(data for _, data in images))


def main() -> None:
    renderer = Renderer()
    BRAND.mkdir(parents=True, exist_ok=True)
    renderer.render(512).save(ICONS / "icon.png", optimize=True)
    renderer.render(256).save(ICONS / "128x128@2x.png", optimize=True)
    renderer.render(128).save(ICONS / "128x128.png", optimize=True)
    renderer.render(32).save(ICONS / "32x32.png", optimize=True)
    write_ico(renderer, ICONS / "icon.ico", [16, 20, 24, 32, 40, 48, 64, 128, 256])
    renderer.render(72).save(BRAND / "mark-72.png", optimize=True)
    # Знак в заголовке окна и в адресной строке — 16–18 px: упрощённый вариант.
    renderer.render(32).save(BRAND / "mark-32.png", optimize=True)
    print("icons written")


if __name__ == "__main__":
    main()
