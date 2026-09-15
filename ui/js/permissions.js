/**
 * Разрешения сайтов: значок, вопрос в окне запроса и название в настройках.
 * Имена — те же, что отдаёт движок (`crates/webview/src/dialogs.rs`).
 */

export const PERMISSIONS = {
  camera: { icon: "camera", ask: "Использовать камеру", name: "Камера" },
  microphone: { icon: "mic", ask: "Использовать микрофон", name: "Микрофон" },
  geolocation: { icon: "location", ask: "Знать, где вы находитесь", name: "Местоположение" },
  notifications: { icon: "alert", ask: "Показывать уведомления", name: "Уведомления" },
  sensors: { icon: "gauge", ask: "Использовать датчики движения", name: "Датчики движения" },
  clipboard: { icon: "clipboard", ask: "Видеть текст и картинки, скопированные в буфер обмена", name: "Буфер обмена" },
  downloads: { icon: "download", ask: "Скачивать несколько файлов подряд", name: "Скачивание нескольких файлов" },
  files: { icon: "document-edit", ask: "Изменять файлы и папки на компьютере", name: "Файлы на компьютере" },
  autoplay: { icon: "play-circle", ask: "Сразу воспроизводить звук и видео", name: "Автовоспроизведение" },
  fonts: { icon: "text-font", ask: "Использовать шрифты, установленные на компьютере", name: "Шрифты компьютера" },
  midi: { icon: "music", ask: "Полностью управлять MIDI-устройствами", name: "MIDI-устройства" },
  windows: { icon: "window-multiple", ask: "Открывать и располагать окна на всех экранах", name: "Окна на всех экранах" },
};

/** «Разрешить в этот раз» — там, где разовое разрешение имеет смысл, как в Chrome. */
export const ONCE = new Set(["camera", "microphone", "geolocation", "sensors", "clipboard", "files", "fonts", "midi", "windows"]);
