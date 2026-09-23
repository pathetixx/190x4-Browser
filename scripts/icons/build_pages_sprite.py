#!/usr/bin/env python3
"""Собрать pages/icons.svg — значки встроенных страниц (новая вкладка).

Значки интерфейса новой вкладки — поиск, меню, правка, монитор ресурсов —
тот же набор 190x4, что и у окна браузера (scripts/icons/build_sprite.py).
Погода — Fluent UI System Icons (Microsoft, MIT): пиктограммы облаков и
осадков рисовать заново незачем, в своей карточке они не спорят с линией
набора. В CSS значки масштабируются под место.

    python3 scripts/icons/build_pages_sprite.py

Погодные значки берутся из уже собранного спрайта. Пересобрать их из Fluent:

    npm pack @fluentui/svg-icons && tar -xzf fluentui-svg-icons-*.tgz
    python3 scripts/icons/build_pages_sprite.py package/icons
"""
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import build_sprite as hud  # noqa: E402

FLUENT = {
    "humidity": "weather_humidity_24_regular",
    "wind": "weather_squalls_24_regular",
    "temperature": "temperature_24_regular",
    "sunny": "weather_sunny_24_regular",
    "moon": "weather_moon_24_regular",
    "partly-day": "weather_partly_cloudy_day_24_regular",
    "partly-night": "weather_partly_cloudy_night_24_regular",
    "cloudy": "weather_cloudy_24_regular",
    "fog": "weather_fog_24_regular",
    "drizzle": "weather_drizzle_24_regular",
    "rain": "weather_rain_24_regular",
    "showers-day": "weather_rain_showers_day_24_regular",
    "showers-night": "weather_rain_showers_night_24_regular",
    "rain-snow": "weather_rain_snow_24_regular",
    "snow": "weather_snow_24_regular",
    "snow-day": "weather_snow_shower_day_24_regular",
    "snow-night": "weather_snow_shower_night_24_regular",
    "thunder": "weather_thunderstorm_24_regular",
    "hail": "weather_hail_day_24_regular",
}

path, circle, dot, cut_rect = hud.path, hud.circle, hud.dot, hud.cut_rect

HUD = {
    "search": hud.ICONS["find"],
    "add": hud.ICONS["tab-add"],
    "more": hud.ICONS["more"],
    "edit": hud.ICONS["edit-16"],
    "delete": hud.ICONS["delete-16"],
    "settings": hud.ICONS["settings"],
    "dismiss": hud.ICONS["stop"],
    "location": hud.ICONS["location"],
    "my-location": circle(10, 10, 5.25) + path("M10 2.75v3M10 14.25v3M2.75 10h3M14.25 10h3") + dot(9.1, 9.1),
    "refresh": hud.ICONS["reload"],
    "cpu": path(cut_rect(5.5, 5.5, 9, 9, tl=2))
    + path("M8.25 8.25h3.5v3.5h-3.5z")
    + path("M8 2.75v2.75M12 2.75v2.75M8 14.5v2.75M12 14.5v2.75M2.75 8h2.75M2.75 12h2.75M14.5 8h2.75M14.5 12h2.75"),
    "ram": path(cut_rect(2.75, 6.25, 14.5, 7, tr=1.5))
    + path("M5.5 8.5h2V11h-2zM9 8.5h2V11H9zM12.5 8.5h2V11h-2z")
    + path("M6 13.25v2M10 13.25v2M14 13.25v2"),
    "gpu": path(cut_rect(2.75, 5.5, 14.5, 9, br=2)) + circle(8.5, 10, 2.5) + path("M13 8.5h2M13 11.5h2"),
    "gauge": hud.ICONS["gauge"],
    "pulse": path("M2.5 10.5h3.25l2-5 3.75 9 2-4h4"),
    "tab": path("M2.5 16.25h15") + path("M4.25 16.25V4.25h8.25l3.25 3.25v8.75"),
    "window": hud.ICONS["window-16"],
    "globe": hud.ICONS["globe"],
}

ORDER = [
    "search", "add", "more", "edit", "delete", "settings", "dismiss", "location", "my-location", "refresh",
    *FLUENT,
    "cpu", "ram", "gpu", "gauge", "pulse", "tab", "window", "globe",
]


def fluent_symbol(name: str, svg: str) -> str:
    view_box = re.search(r'viewBox="([^"]+)"', svg).group(1)
    body = re.sub(r"^.*?<svg[^>]*>|</svg>\s*$", "", svg, flags=re.S)
    body = re.sub(r'\sfill="#[0-9a-fA-F]{3,6}"', ' fill="currentColor"', body)
    return f'<symbol id="i-{name}" viewBox="{view_box}">{body.strip()}</symbol>'


def main() -> None:
    out = Path(__file__).resolve().parents[2] / "pages" / "icons.svg"
    if len(sys.argv) > 1:
        source = Path(sys.argv[1])
        fluent = {name: fluent_symbol(name, (source / f"{file}.svg").read_text()) for name, file in FLUENT.items()}
    else:
        built = out.read_text()
        fluent = {}
        for name in FLUENT:
            found = re.search(rf'<symbol id="i-{re.escape(name)}".*?</symbol>', built, flags=re.S)
            if not found:
                sys.exit(f"в {out} нет значка {name}: соберите из пакета Fluent (см. справку)")
            fluent[name] = found.group(0)

    symbols = [fluent[name] if name in fluent else hud.symbol(name, HUD[name], 20) for name in ORDER]
    out.write_text(
        '<svg xmlns="http://www.w3.org/2000/svg" style="display:none">\n' + "\n".join(symbols) + "\n</svg>\n"
    )
    print(f"{len(symbols)} icons -> {out}")


if __name__ == "__main__":
    main()
