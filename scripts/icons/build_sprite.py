#!/usr/bin/env python3
"""Собрать ui/assets/icons.svg — набор иконок 190x4.

Иконки нарисованы здесь же, в коде, языком HUD новой вкладки: тонкая линия
по сетке 20×20 (рисунок — в квадрате 3…17), прямые концы и острые стыки,
вместо скруглений — срезанный угол, вместо точек — квадраты и ромбы.

Один рисунок служит всем размерам: у символа на 20, 16 и 12 px своё окно
просмотра (поля подрезаны, чтобы значок заполнял место, как в любом наборе)
и своя толщина линии — на экране она выходит около 1,3 px на 20 и 16 px и
чуть толще на 12, чтобы крестик вкладки не таял.

Залитые варианты (для нажатых кнопок) — тот же контур и полупрозрачная
заливка формы; звезда закладки заливается целиком.

Запуск:
    python3 scripts/icons/build_sprite.py
"""
import math
from pathlib import Path

# Окно просмотра и толщина линии (в единицах сетки) для каждого размера.
SIZES = {
    20: ("1 1 18 18", 1.15),
    16: ("1.5 1.5 17 17", 1.25),
    12: ("2.5 2.5 15 15", 1.6),
}


def num(value: float) -> str:
    return f"{round(value, 2):g}"


def pts(points) -> str:
    return " ".join(f"{num(x)} {num(y)}" for x, y in points)


def poly(points, close=True) -> str:
    d = f"M{pts(points[:1])}L{pts(points[1:])}"
    return d + ("Z" if close else "")


def cut_rect(x, y, w, h, tl=0.0, tr=0.0, br=0.0, bl=0.0) -> str:
    """Прямоугольник со срезанными углами — основная форма набора."""
    points = [
        (x + tl, y),
        (x + w - tr, y),
        (x + w, y + tr),
        (x + w, y + h - br),
        (x + w - br, y + h),
        (x + bl, y + h),
        (x, y + h - bl),
        (x, y + tl),
    ]
    deduped = []
    for point in points:
        if not deduped or deduped[-1] != point:
            deduped.append(point)
    if deduped[0] == deduped[-1]:
        deduped.pop()
    return poly(deduped)


def octagon(cx, cy, apothem) -> str:
    """Восьмиугольник с плоскими сторонами сверху, снизу и по бокам."""
    radius = apothem / math.cos(math.radians(22.5))
    return poly(
        [
            (cx + radius * math.cos(math.radians(22.5 + 45 * k)), cy + radius * math.sin(math.radians(22.5 + 45 * k)))
            for k in range(8)
        ]
    )


def star(cx, cy, outer, inner) -> str:
    points = []
    for k in range(10):
        radius = outer if k % 2 == 0 else inner
        angle = math.radians(-90 + 36 * k)
        points.append((cx + radius * math.cos(angle), cy + radius * math.sin(angle)))
    return poly(points)


def gear(cx, cy, root, tip, teeth=8) -> str:
    """Шестерня с трапециевидными зубьями: угловатая, как всё в наборе."""
    points = []
    step = 360 / teeth
    for k in range(teeth):
        a = -90 + step * k
        for angle, radius in ((a - 14, root), (a - 8, tip), (a + 8, tip), (a + 14, root)):
            points.append((cx + radius * math.cos(math.radians(angle)), cy + radius * math.sin(math.radians(angle))))
    return poly(points)


def path(d: str) -> str:
    return f'<path d="{d}"/>'


def circle(cx, cy, r) -> str:
    return f'<circle cx="{num(cx)}" cy="{num(cy)}" r="{num(r)}"/>'


def dot(x, y, size=1.8) -> str:
    """Квадрат-точка: вместо круглых точек у набора квадраты."""
    return f'<rect x="{num(x)}" y="{num(y)}" width="{num(size)}" height="{num(size)}" fill="currentColor" stroke="none"/>'


def fill(d: str, opacity=0.25) -> str:
    """Заливка формы под контуром — для нажатых вариантов."""
    return f'<path d="{d}" fill="currentColor" fill-opacity="{opacity}" stroke="none"/>'


def solid(d: str) -> str:
    return f'<path d="{d}" fill="currentColor" stroke="none"/>'


# ── Формы, которые повторяются ──────────────────────────────────────

ARROW_BACK = path("M16.75 10H3.75M8.75 5 3.75 10l5 5")
ARROW_FORWARD = path("M3.25 10h13M11.25 5l5 5-5 5")
RELOAD = path("M16.5 10a6.5 6.5 0 1 1-1.9-4.6L16.5 7.3") + path("M16.5 3.3v4h-4")
HISTORY_ARROW = path("M3.5 10a6.5 6.5 0 1 0 1.9-4.6L3.5 7.3") + path("M3.5 3.3v4h4")
HISTORY = HISTORY_ARROW + path("M10 6.75v3.75h3")
DISMISS = path("M5 5l10 10M15 5 5 15")
PLUS = path("M10 4.5v11M4.5 10h11")
DOWNLOAD = path("M10 3.25V13M6.25 9.25 10 13l3.75-3.75") + path("M3.75 13.5v3h12.5v-3")
SHIELD_SHAPE = poly([(10, 2.75), (16.25, 5), (16.25, 10.5), (10, 17.25), (3.75, 10.5), (3.75, 5)])
STAR_SHAPE = star(10, 10.75, 6.9, 2.95)
MAGNIFIER = circle(8.75, 8.75, 5.5) + path("M12.75 12.75 16.75 16.75")
GLOBE = circle(10, 10, 6.75) + path("M3.25 10h13.5") + path(
    "M10 3.25c-2 1.75-3 4-3 6.75s1 5 3 6.75M10 3.25c2 1.75 3 4 3 6.75s-1 5-3 6.75"
)
FOLDER = path("M2.75 4.25H8l2 2h7.25v8.25L16 15.75H2.75z") + path("M2.75 8.25h14.5")
FOLDER_OPEN = path("M2.75 15.75V4.25H8l2 2h5.5v2.5") + path("M2.75 15.75 5.25 9.25h12.25L15 15.75z")
DOCUMENT_SHAPE = "M5 2.75h6.25L15 6.5v10.75H5z"
KEY = circle(6.5, 13.5, 3.25) + path("M8.8 11.2 16.5 3.5M14 6l2.25 2.25M11.75 8.25l1.75 1.75")
TRANSLATE = path("M2.75 15 5.5 5h1.25L9.5 15M3.8 11.5h4.65") + path(
    "M16.75 6.5V15M16.75 6.5h-3.5L12 7.75V10l1.25 1.25h3.5M14.5 11.25 12 15"
)
TRANSLATE_PLATE = cut_rect(1.75, 3.25, 16.5, 13.5, tr=3, bl=3)
GEAR_SHAPE = gear(10, 10, 5.4, 7.4)
GEAR = path(GEAR_SHAPE) + circle(10, 10, 2.25)
INFO = path(octagon(10, 10, 7)) + path("M10 9v5") + dot(9.1, 5.6)
OPEN = path("M9 3.75H3.75v12.5h12.5V11") + path("M11.25 3.75h5v5M16.25 3.75 9.5 10.5")
MORE = dot(3.6, 9.1) + dot(9.1, 9.1) + dot(14.6, 9.1)
SPEAKER_BODY = "M3 7.5h3l4-3.25v11.5L6 12.5H3z"
VIDEO_FRAME = cut_rect(2.75, 4.25, 14.5, 11.5, tr=3, bl=3)
VIDEO_PLAY = "M8.25 7.5v5l4.25-2.5z"
PIN_BODY = "M8 3.5h4v4.75L14.5 11h-9L8 8.25z"
FAVORITES_STAR = star(7.25, 8.4, 4.9, 2.05)
LOCK = path("M6.75 8.5V6.25a3.25 3.25 0 0 1 6.5 0V8.5") + path(cut_rect(4.5, 8.5, 11, 8.25, br=2)) + path(
    "M10 11.25v2.5"
)


ICONS = {
    # ── 20: панель инструментов, рельс, меню, настройки ──
    "back": ARROW_BACK,
    "forward": ARROW_FORWARD,
    "reload": RELOAD,
    "stop": DISMISS,
    "home": path("M3.25 9.25 10 3.5l6.75 5.75") + path("M5.25 7.75v8.5h9.5v-8.5") + path("M8.25 16.25v-4.5h3.5v4.5"),
    "more": MORE,
    "download": DOWNLOAD,
    "puzzle": path("M3.5 7h3.5V4.5h3V7h3.5v3.5H16v3h-2.5V17h-10z"),
    "video": path(VIDEO_FRAME) + path(VIDEO_PLAY),
    "video-filled": fill(VIDEO_FRAME) + path(VIDEO_FRAME) + solid(VIDEO_PLAY) + path(VIDEO_PLAY),
    # Автопролистывание ленты: экран телефона и стрелка вниз.
    "autoscroll": path(cut_rect(5.5, 2.75, 9, 14.5, tr=2)) + path("M10 6.75v6.5M7.75 11 10 13.25 12.25 11"),
    "autoscroll-filled": fill(cut_rect(5.5, 2.75, 9, 14.5, tr=2))
    + path(cut_rect(5.5, 2.75, 9, 14.5, tr=2))
    + path("M10 6.75v6.5M7.75 11 10 13.25 12.25 11"),
    # Пропустить сегмент (SponsorBlock): треугольник «играть» и черта конца.
    "skip": path("M4.75 4.75v10.5L12.25 10z") + path("M15.25 4.75v10.5"),
    # Трансляция: окно чата со срезанным углом и хвостиком, в нём две риски
    # эфира — значок расширения Twitch.
    "stream": path("M3.25 3.25h13.5v8.5l-3.5 3.5h-3.5l-2.5 2v-2h-4z") + path("M8.75 6.75v3.75M12.75 6.75v3.75"),
    "shield": path(SHIELD_SHAPE),
    "shield-filled": fill(SHIELD_SHAPE, 0.3) + path(SHIELD_SHAPE),
    "favorites": path(FAVORITES_STAR) + path("M13.5 7.5h3M13 11h3.5M4 15.5h12.5"),
    "favorites-filled": solid(FAVORITES_STAR) + path(FAVORITES_STAR) + path("M13.5 7.5h3M13 11h3.5M4 15.5h12.5"),
    "history": HISTORY,
    "history-filled": fill("M3.5 10a6.5 6.5 0 1 0 13 0a6.5 6.5 0 1 0-13 0z", 0.3) + HISTORY,
    "translate": TRANSLATE,
    "translate-filled": fill(TRANSLATE_PLATE, 0.22) + TRANSLATE,
    "settings": GEAR,
    "settings-filled": fill(GEAR_SHAPE, 0.3) + GEAR,
    "tab-add": PLUS,
    "zoom-in": MAGNIFIER + path("M8.75 6.5V11M6.5 8.75H11"),
    "zoom-out": MAGNIFIER + path("M6.5 8.75H11"),
    "print": path("M6 7.25V3.5h8v3.75")
    + path("M6 14.25H3.5v-7h13v7H14")
    + path("M6 11.5h8v5H6z")
    + dot(13.25, 8.75, 1.5),
    "find": MAGNIFIER,
    "code": path("M6.75 5.75 2.5 10l4.25 4.25M13.25 5.75 17.5 10l-4.25 4.25M11.25 4 8.75 16"),
    "info": INFO,
    "exit": path("M8.5 3.5h-5v13h5") + path("M8 10h8.5M13.5 6.75 16.75 10l-3.25 3.25"),
    "key": KEY,
    "paint": path("M16.75 3.25 11 9") + path("M9.25 7.25l3.5 3.5") + path("M9.25 7.25 5 11.5l-1.75 5.25L8.5 15l4.25-4.25"),
    "power": path("M10 3.25V9.5") + path("M6.25 5.5a6 6 0 1 0 7.5 0"),
    "globe": GLOBE,
    "lock-shield": path(SHIELD_SHAPE)
    + path("M8.5 9.5V8.25a1.5 1.5 0 0 1 3 0V9.5")
    + path("M7.5 9.5h5V13h-5z"),
    "language": TRANSLATE,
    "folder": FOLDER,
    "folder-open": FOLDER_OPEN,
    "document": path(DOCUMENT_SHAPE),
    "doc-text": path(DOCUMENT_SHAPE) + path("M7.5 9.75h5M7.5 12.25h5M7.5 14.75h3"),
    "image": path(cut_rect(3, 3.5, 14, 13, tr=3))
    + path("M3 13.75l4.25-4.25 3.5 3.5 2-2 4.25 4.25")
    + dot(11.5, 6.25, 2),
    "music": path("M7.5 14.25V4.5l9-1.75v9.75M7.5 7.5l9-1.75") + circle(5.6, 14.25, 1.9) + circle(14.6, 12.5, 1.9),
    "archive": path("M2.75 3.75h14.5v3.5H2.75z") + path("M4 7.25v9h12v-9") + path("M8.25 10.25h3.5"),
    "app": path(cut_rect(3, 3.5, 14, 13, tr=3)) + path("M3 7h14") + dot(4.75, 4.6, 1.3) + dot(6.9, 4.6, 1.3),
    "broom": path("M16.75 3.25 10.5 9.5") + path("M9 8l3 3-3.5 5.75-5.25-5.25z") + path("M10.5 9.5 6.7 13.3"),
    "import": path("M12.5 3.5h4v13h-4") + path("M3.5 10h8.25M8.5 6.75 11.75 10 8.5 13.25"),
    "export": path("M7.5 3.5h-4v13h4") + path("M7.75 10h8.75M13.25 6.75 16.5 10l-3.25 3.25"),
    "star-20": path(STAR_SHAPE),
    "star-20-filled": solid(STAR_SHAPE) + path(STAR_SHAPE),
    # ── окна страниц: запросы разрешений и открытие приложений ──
    "camera": path("M2.75 6.25h3.5l1.5-2h4.5l1.5 2h3.5v10H2.75z") + circle(10, 11, 3) + dot(14, 7.75, 1.5),
    "mic": path("M8.25 2.75h3.5L13 4v6l-1.25 1.25h-3.5L7 10V4z")
    + path("M4.75 9.25V10c0 2.9 2.35 5 5.25 5s5.25-2.1 5.25-5v-.75")
    + path("M10 15v2.5M7.25 17.5h5.5"),
    "location": path("M10 17.25 4.75 11V6.5l2.5-3.25h5.5l2.5 3.25V11z") + path("M10 5.75l2 2-2 2-2-2z"),
    "alert": path("M4.5 14.75h11") + path("M6 14.75v-6L8 5.5h4l2 3.25v6") + path("M10 3v2.5M8.5 17.25h3"),
    "clipboard": path("M7 4.25H4.5v13h11v-13H13") + path("M7.25 2.75h5.5v3h-5.5z"),
    "gauge": path("M4.37 14.25A6.5 6.5 0 1 1 15.63 14.25") + path("M10 11l3.25-3.75") + dot(9.25, 10.25, 1.5),
    "document-edit": path("M11 17.25H4.5V2.75h6L14 6.25v2.5") + path("M11.75 17.25V15l4.75-4.75 2.25 2.25-4.75 4.75z"),
    "play-circle": path(octagon(10, 10, 7)) + path("M8.5 7v6l5-3z"),
    "text-font": path("M4.75 16 9.25 4h1.5l4.5 12M6.5 12h7"),
    "window-multiple": path(cut_rect(3, 6.5, 10.5, 10.5, tr=2)) + path("M6.5 6.5V3h10.5v10.5h-3.5"),
    "open": OPEN,
    # ── 16: адресная строка, вкладки, строки списков ──
    "star-16": path(STAR_SHAPE),
    "star-16-filled": solid(STAR_SHAPE) + path(STAR_SHAPE),
    "key-16": KEY,
    "translate-16": TRANSLATE,
    "zoom-16": MAGNIFIER + path("M8.75 6.5V11M6.5 8.75H11"),
    "shield-16": path(SHIELD_SHAPE),
    "search-16": MAGNIFIER,
    "globe-16": GLOBE,
    "speaker-16": path(SPEAKER_BODY) + path("M12.75 7.5 14.5 10l-1.75 2.5M15.25 5.25 17.5 10l-2.25 4.75"),
    "mute-16": path(SPEAKER_BODY) + path("M13 8l4 4M17 8l-4 4"),
    "dismiss-16": DISMISS,
    "add-16": PLUS,
    "chevron-down-16": path("M5.5 7.75 10 12.25l4.5-4.5"),
    "chevron-up-16": path("M5.5 12.25 10 7.75l4.5 4.5"),
    "chevron-right-16": path("M7.75 5.5 12.25 10l-4.5 4.5"),
    "chevron-double-16": path("M5 5.5 9.5 10 5 14.5M10.5 5.5 15 10l-4.5 4.5"),
    "folder-16": FOLDER,
    "folder-open-16": FOLDER_OPEN,
    "eye-16": path("M2.5 10 6.25 5.5h7.5L17.5 10l-3.75 4.5h-7.5z") + circle(10, 10, 2.25),
    "eye-off-16": path("M2.5 10 6.25 5.5h7.5L17.5 10l-3.75 4.5h-7.5z") + circle(10, 10, 2.25) + path("M3.75 3.75l12.5 12.5"),
    "copy-16": path(cut_rect(3.5, 6.25, 9, 10.75, tr=2)) + path("M7 6.25V3h7.5l2 2v9h-3.5"),
    # Поменять местами: стрелка вправо сверху, влево снизу.
    "swap-16": path("M3.25 7h13M13 3.75 16.25 7 13 10.25") + path("M16.75 13h-13M7 9.75 3.75 13 7 16.25"),
    "edit-16": path("M3.75 16.25V13L13 3.75 16.25 7 7 16.25z") + path("M11 5.75 14.25 9"),
    "delete-16": path("M3.25 5.25h13.5M7.75 5.25v-2h4.5v2")
    + path("M5 5.25l.75 11.5h8.5l.75-11.5")
    + path("M8.5 8.5v5.25M11.5 8.5v5.25"),
    "pause-16": path("M5.75 4.5H8.5v11H5.75zM11.5 4.5h2.75v11H11.5z"),
    "play-16": path("M6.5 4.25v11.5L15.75 10z"),
    "retry-16": RELOAD,
    "open-16": OPEN,
    "checkmark-16": path("M4 10.25 8 14.25l8-8.5"),
    "more-16": MORE,
    "history-16": HISTORY,
    "document-16": path(DOCUMENT_SHAPE),
    "link-16": path("M8.25 11.75l3.5-3.5")
    + path("M9.25 6.5l1.5-1.5a3.2 3.2 0 0 1 4.5 4.5l-1.5 1.5M10.75 13.5l-1.5 1.5a3.2 3.2 0 0 1-4.5-4.5l1.5-1.5"),
    "info-16": INFO,
    "download-16": DOWNLOAD,
    "lock-16": LOCK,
    "pin-16": path("M6.5 3.5h7") + path(PIN_BODY) + path("M10 11v5.75"),
    "pin-16-filled": path("M6.5 3.5h7") + solid(PIN_BODY) + path(PIN_BODY) + path("M10 11v5.75"),
    "person-16": circle(10, 6.5, 3) + path("M4 17v-1.25l2.25-3.25h7.5L16 15.75V17"),
    "warning-16": path("M10 3 17.5 16.5h-15z") + path("M10 8v4") + dot(9.1, 13.3),
    # ── 16: окна, разделённый экран, приватное окно, группа ──
    "window-16": path(cut_rect(3, 4, 14, 12, tr=2.5)) + path("M3 7.5h14"),
    "split-16": path(cut_rect(2.75, 4, 14.5, 12, tr=2.5)) + path("M10 4v12"),
    "private-16": path("M2.5 9.5h15M4.75 9.5 6.5 4h7l1.75 5.5")
    + circle(6, 13.25, 2.25)
    + circle(14, 13.25, 2.25)
    + path("M8.25 13h3.5"),
    "tab-group-16": path("M2.75 4.75h6.5l1.5 2h6.5v8.5H2.75z") + path("M10 9.25l1.75 1.75L10 12.75 8.25 11z"),
}

# Крестик вкладки и мелкие шевроны — 12 px: та же форма, линия толще.
SMALL = {
    "dismiss-12": path("M5.5 5.5l9 9M14.5 5.5l-9 9"),
    "chevron-down-12": path("M5.5 8 10 12.5 14.5 8"),
    "chevron-right-12": path("M8 5.5 12.5 10 8 14.5"),
}

# Кнопки окна рисуем по геометрии Windows 11 (Segoe Fluent Icons): глиф
# 10×10, линия в один пиксель. Кнопки окна должны выглядеть как в любом
# другом приложении системы.
CAPTION = {
    "cap-min": '<path d="M0 5.5h10" stroke="currentColor" stroke-width="1" fill="none"/>',
    "cap-max": '<rect x="0.5" y="0.5" width="9" height="9" rx="1" stroke="currentColor" stroke-width="1" fill="none"/>',
    "cap-restore": '<path d="M2.5 2.5V1.5a1 1 0 0 1 1-1h5a1 1 0 0 1 1 1v5a1 1 0 0 1-1 1h-1" stroke="currentColor" stroke-width="1" fill="none"/>'
    '<rect x="0.5" y="2.5" width="7" height="7" rx="1" stroke="currentColor" stroke-width="1" fill="none"/>',
    "cap-close": '<path d="M0.5 0.5l9 9M9.5 0.5l-9 9" stroke="currentColor" stroke-width="1" fill="none"/>',
}


def symbol(alias: str, body: str, size: int) -> str:
    view_box, stroke = SIZES[size]
    return (
        f'<symbol id="i-{alias}" viewBox="{view_box}">'
        f'<g fill="none" stroke="currentColor" stroke-width="{num(stroke)}" stroke-linecap="butt" '
        f'stroke-linejoin="miter" stroke-miterlimit="8">{body}</g></symbol>'
    )


def size_of(alias: str) -> int:
    return 16 if alias.endswith(("-16", "-16-filled")) else 20


def main() -> None:
    out = Path(__file__).resolve().parents[2] / "ui" / "assets" / "icons.svg"
    parts = ['<svg xmlns="http://www.w3.org/2000/svg">']
    for alias, body in ICONS.items():
        parts.append(symbol(alias, body, size_of(alias)))
    for alias, body in SMALL.items():
        parts.append(symbol(alias, body, 12))
    for alias, body in CAPTION.items():
        parts.append(f'<symbol id="i-{alias}" viewBox="0 0 10 10">{body}</symbol>')
    parts.append("</svg>\n")
    out.write_text("\n".join(parts))
    total = len(ICONS) + len(SMALL) + len(CAPTION)
    print(f"{total} иконок → {out} ({out.stat().st_size // 1024} КБ)")


if __name__ == "__main__":
    main()
