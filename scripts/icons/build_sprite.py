#!/usr/bin/env python3
"""Собрать ui/assets/icons.svg из Fluent UI System Icons.

Иконки интерфейса — Fluent UI System Icons (Microsoft, MIT): тот же набор,
на котором построен Edge. Каждая нарисована под свой размер (12/16/20 px),
поэтому в спрайт кладём ровно те размеры, в которых иконка показывается, —
масштабированная двадцатка на 16 px мылится.

Запуск:
    npm pack @fluentui/svg-icons && tar -xzf fluentui-svg-icons-*.tgz
    python3 scripts/icons/build_sprite.py package/icons
"""
import re
import sys
from pathlib import Path

ICONS = {
    # 20 — тулбар, рельс, меню, настройки
    "back": "arrow_left_20_regular",
    "forward": "arrow_right_20_regular",
    "reload": "arrow_clockwise_20_regular",
    "stop": "dismiss_20_regular",
    "home": "home_20_regular",
    "more": "more_horizontal_20_regular",
    "download": "arrow_download_20_regular",
    "puzzle": "puzzle_piece_20_regular",
    "video": "video_clip_20_regular",
    "video-filled": "video_clip_20_filled",
    "shield": "shield_20_regular",
    "shield-filled": "shield_20_filled",
    "favorites": "star_line_horizontal_3_20_regular",
    "favorites-filled": "star_line_horizontal_3_20_filled",
    "history": "history_20_regular",
    "history-filled": "history_20_filled",
    "translate": "translate_20_regular",
    "translate-filled": "translate_20_filled",
    "settings": "settings_20_regular",
    "settings-filled": "settings_20_filled",
    "tab-add": "add_20_regular",
    "zoom-in": "zoom_in_20_regular",
    "zoom-out": "zoom_out_20_regular",
    "print": "print_20_regular",
    "find": "search_20_regular",
    "code": "code_20_regular",
    "info": "info_20_regular",
    "exit": "arrow_exit_20_regular",
    "key": "key_20_regular",
    "paint": "paint_brush_20_regular",
    "power": "power_20_regular",
    "globe": "globe_20_regular",
    "lock-shield": "lock_shield_20_regular",
    "language": "local_language_20_regular",
    "folder": "folder_20_regular",
    "folder-open": "folder_open_20_regular",
    "document": "document_20_regular",
    "doc-text": "document_text_20_regular",
    "image": "image_20_regular",
    "music": "music_note_2_20_regular",
    "archive": "archive_20_regular",
    "app": "app_generic_20_regular",
    "broom": "broom_20_regular",
    "import": "arrow_import_20_regular",
    "export": "arrow_export_20_regular",
    "star-20": "star_20_regular",
    "star-20-filled": "star_20_filled",
    # окна страниц: запросы разрешений и открытие приложений
    "camera": "camera_20_regular",
    "mic": "mic_20_regular",
    "location": "location_20_regular",
    "alert": "alert_20_regular",
    "clipboard": "clipboard_20_regular",
    "gauge": "gauge_20_regular",
    "document-edit": "document_edit_20_regular",
    "play-circle": "play_circle_20_regular",
    "text-font": "text_font_20_regular",
    "window-multiple": "window_multiple_20_regular",
    "open": "open_20_regular",
    # 16 — адресная строка, вкладки, строки списков
    "star-16": "star_16_regular",
    "star-16-filled": "star_16_filled",
    "key-16": "key_16_regular",
    "translate-16": "translate_16_regular",
    "zoom-16": "zoom_in_16_regular",
    "shield-16": "shield_16_regular",
    "search-16": "search_16_regular",
    "globe-16": "globe_16_regular",
    "speaker-16": "speaker_2_16_regular",
    "mute-16": "speaker_mute_16_regular",
    "dismiss-16": "dismiss_16_regular",
    "add-16": "add_16_regular",
    "chevron-down-16": "chevron_down_16_regular",
    "chevron-up-16": "chevron_up_16_regular",
    "chevron-right-16": "chevron_right_16_regular",
    "chevron-double-16": "chevron_double_right_16_regular",
    "folder-16": "folder_16_regular",
    "folder-open-16": "folder_open_16_regular",
    "eye-16": "eye_16_regular",
    "eye-off-16": "eye_off_16_regular",
    "copy-16": "copy_16_regular",
    "edit-16": "edit_16_regular",
    "delete-16": "delete_16_regular",
    "pause-16": "pause_16_regular",
    "play-16": "play_16_regular",
    "retry-16": "arrow_clockwise_16_regular",
    "open-16": "open_16_regular",
    "checkmark-16": "checkmark_16_regular",
    "more-16": "more_horizontal_16_regular",
    "history-16": "history_16_regular",
    "document-16": "document_16_regular",
    "link-16": "link_16_regular",
    "info-16": "info_16_regular",
    "download-16": "arrow_download_16_regular",
    "lock-16": "lock_closed_16_regular",
    "pin-16": "pin_16_regular",
    "pin-16-filled": "pin_16_filled",
    "person-16": "person_16_regular",
    "warning-16": "warning_16_regular",
    # 12 — крестик вкладки, мелкие шевроны
    "dismiss-12": "dismiss_12_regular",
    "chevron-down-12": "chevron_down_12_regular",
    "chevron-right-12": "chevron_right_12_regular",
}

# Кнопки окна рисуем сами по геометрии Windows 11 (Segoe Fluent Icons):
# глиф 10×10, линия в один пиксель. Иконки из набора здесь не годятся —
# кнопки окна должны выглядеть как в любом другом приложении системы.
CAPTION = {
    "cap-min": '<path d="M0 5.5h10" stroke="currentColor" stroke-width="1" fill="none"/>',
    "cap-max": '<rect x="0.5" y="0.5" width="9" height="9" rx="1" stroke="currentColor" stroke-width="1" fill="none"/>',
    "cap-restore": '<path d="M2.5 2.5V1.5a1 1 0 0 1 1-1h5a1 1 0 0 1 1 1v5a1 1 0 0 1-1 1h-1" stroke="currentColor" stroke-width="1" fill="none"/>'
    '<rect x="0.5" y="2.5" width="7" height="7" rx="1" stroke="currentColor" stroke-width="1" fill="none"/>',
    "cap-close": '<path d="M0.5 0.5l9 9M9.5 0.5l-9 9" stroke="currentColor" stroke-width="1" fill="none"/>',
}


def symbol(alias: str, source: str) -> str:
    view_box = re.search(r'viewBox="([^"]+)"', source).group(1)
    body = re.sub(r"^.*?<svg[^>]*>|</svg>\s*$", "", source.strip(), flags=re.S)
    body = re.sub(r'fill="#[0-9A-Fa-f]{3,8}"', 'fill="currentColor"', body)
    return f'<symbol id="i-{alias}" viewBox="{view_box}">{body.strip()}</symbol>'


def main() -> None:
    root = Path(sys.argv[1] if len(sys.argv) > 1 else "package/icons")
    out = Path(__file__).resolve().parents[2] / "ui" / "assets" / "icons.svg"

    parts = ['<svg xmlns="http://www.w3.org/2000/svg">']
    for alias, name in ICONS.items():
        parts.append(symbol(alias, (root / f"{name}.svg").read_text()))
    for alias, body in CAPTION.items():
        parts.append(f'<symbol id="i-{alias}" viewBox="0 0 10 10">{body}</symbol>')
    parts.append("</svg>\n")

    out.write_text("\n".join(parts))
    print(f"{len(ICONS) + len(CAPTION)} иконок → {out} ({out.stat().st_size // 1024} КБ)")


if __name__ == "__main__":
    main()
