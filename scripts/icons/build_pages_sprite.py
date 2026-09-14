#!/usr/bin/env python3
"""Собрать pages/icons.svg — значки встроенных страниц (новая вкладка).

Тот же набор, что у интерфейса: Fluent UI System Icons (Microsoft, MIT). Все
значки — 24 px, в CSS они масштабируются под место.

    npm pack @fluentui/svg-icons && tar -xzf fluentui-svg-icons-*.tgz
    python3 scripts/icons/build_pages_sprite.py package/icons
"""
import re
import sys
from pathlib import Path

ICONS = {
    "search": "search_24_regular",
    "add": "add_24_regular",
    "more": "more_horizontal_24_regular",
    "edit": "edit_24_regular",
    "delete": "delete_24_regular",
    "settings": "settings_24_regular",
    "dismiss": "dismiss_24_regular",
    "location": "location_24_regular",
    "my-location": "my_location_24_regular",
    "refresh": "arrow_clockwise_24_regular",
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
    "cpu": "developer_board_24_regular",
    "ram": "ram_20_regular",
    "gpu": "video_24_regular",
    "gauge": "top_speed_24_regular",
    "pulse": "pulse_24_regular",
    "tab": "tab_desktop_24_regular",
    "window": "window_24_regular",
    "globe": "globe_24_regular",
}


def main() -> None:
    source = Path(sys.argv[1] if len(sys.argv) > 1 else "package/icons")
    out = Path(__file__).resolve().parents[2] / "pages" / "icons.svg"
    symbols = []
    for name, file in ICONS.items():
        svg = (source / f"{file}.svg").read_text()
        view_box = re.search(r'viewBox="([^"]+)"', svg).group(1)
        body = re.sub(r"^.*?<svg[^>]*>|</svg>\s*$", "", svg, flags=re.S)
        body = re.sub(r'\sfill="#[0-9a-fA-F]{3,6}"', ' fill="currentColor"', body)
        symbols.append(f'<symbol id="i-{name}" viewBox="{view_box}">{body.strip()}</symbol>')
    out.write_text(
        '<svg xmlns="http://www.w3.org/2000/svg" style="display:none">\n'
        + "\n".join(symbols)
        + "\n</svg>\n"
    )
    print(f"{len(symbols)} icons -> {out}")


if __name__ == "__main__":
    main()
