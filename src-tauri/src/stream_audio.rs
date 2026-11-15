//! Звук для трансляций: демонстрация экрана Discord и захват звука
//! приложения в OBS слышат то, что играет браузер.
//!
//! Звук страниц играют процессы движка WebView2 (`msedgewebview2.exe`), а не
//! процесс браузера. Discord и OBS берут звук только у процесса окна, которое
//! захватывают, — и не слышат ничего. У Microsoft это открытый запрос
//! (WebView2Feedback #2236), своего решения у движка нет.
//!
//! Браузер пересылает звук сам: снимает звук дерева процессов движка захватом
//! звука процесса (он есть с Windows 11) и проигрывает его из своего
//! процесса на устройство по умолчанию. Сессии движка в микшере на это время
//! приглушаются почти до нуля — иначе звук шёл бы дважды. Выключить их совсем
//! нельзя: захват берёт поток уже после громкости микшера, и выключенный звук
//! был бы тишиной и в пересылке. Приглушённый звук возвращается к прежнему
//! уровню усилением.
//!
//! Громкость микшера Windows помнит между запусками. Пока сессии приглушены,
//! в профиле лежит отметка: браузер, закрывшийся аварийно, при следующем
//! запуске возвращает сессиям движка громкость.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tauri::{AppHandle, Emitter, Manager};
use windows::core::{implement, Interface, GUID, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::{
    eConsole, eRender, ActivateAudioInterfaceAsync, IActivateAudioInterfaceAsyncOperation,
    IActivateAudioInterfaceCompletionHandler, IActivateAudioInterfaceCompletionHandler_Impl,
    IAudioCaptureClient, IAudioClient, IAudioRenderClient, IAudioSessionControl2,
    IAudioSessionManager2, IMMDeviceEnumerator, ISimpleAudioVolume, MMDeviceEnumerator,
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_LOOPBACK,
    AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, AUDIOCLIENT_ACTIVATION_PARAMS,
    AUDIOCLIENT_ACTIVATION_PARAMS_0, AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
    AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS, DEVICE_STATE_ACTIVE,
    PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE, VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
    WAVEFORMATEX,
};
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, BLOB, CLSCTX_ALL,
    COINIT_MULTITHREADED,
};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows::Win32::System::Variant::VT_BLOB;

use crate::state::App;

/// Настройка «Звук для трансляций».
pub const SETTING: &str = "stream_audio";

/// Отметка в профиле: сессии движка приглушены и ещё не возвращены.
const MARKER: &str = "stream-audio.lowered";

/// До скольки приглушаются сессии движка (−60 дБ) и во сколько раз пересылка
/// усиливает снятый звук. Захват идёт в 32-битных float, и такое усиление
/// звук не портит.
const QUIET: f32 = 0.001;
/// Громкость сессии ниже этой — приглушена браузером, а не человеком.
const LOWERED: f32 = 0.01;

const RATE: u32 = 48_000;
const CHANNELS: u16 = 2;
/// Буфер захвата 200 мс, вывода — 100 мс (в сотнях наносекунд).
const CAPTURE_BUFFER: i64 = 2_000_000;
const RENDER_BUFFER: i64 = 1_000_000;
/// Больше этого пересылка не копит: звук не должен отставать от картинки.
const BACKLOG_FRAMES: usize = (RATE as usize) / 4;
/// Как часто ищутся новые сессии движка.
const HOUSEKEEPING: Duration = Duration::from_millis(250);
/// Как часто проверяются процессы движка и устройство вывода по умолчанию.
const PROCESSES: Duration = Duration::from_secs(2);
/// Сколько после аварийного выхода браузер ищет сессии, чтобы вернуть им громкость.
const REPAIR_FOR: Duration = Duration::from_secs(10 * 60);

const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;

struct Running {
    stop: Arc<AtomicBool>,
    thread: std::thread::JoinHandle<()>,
}

static RUNNING: Mutex<Option<Running>> = Mutex::new(None);

/// Включить или выключить пересылку по настройке. Зовётся при запуске и при
/// смене настройки.
pub fn apply(app: &AppHandle) {
    let enabled = app.state::<App>().store.setting_bool(SETTING, false);
    let mut running = RUNNING.lock();
    // Прежний поток останавливается сам: он возвращает громкость и выходит.
    // Ждёт его новый поток, а не вызывающий — тот может быть главным.
    let previous = running.take().map(|old| {
        old.stop.store(true, Ordering::SeqCst);
        old.thread
    });
    let marker = crate::profile_dir().join(MARKER);
    if !enabled && !marker.exists() && previous.is_none() {
        return;
    }
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let handle = app.clone();
    let spawned = std::thread::Builder::new()
        .name("stream-audio".into())
        .spawn(move || {
            if let Some(previous) = previous {
                let _ = previous.join();
            }
            if enabled {
                relay(&handle, &flag, &marker);
            } else if marker.exists() {
                repair(&flag, &marker);
            }
        });
    match spawned {
        Ok(thread) => *running = Some(Running { stop, thread }),
        Err(err) => tracing::warn!(%err, "поток пересылки звука не запущен"),
    }
}

/// Выход из браузера: громкость сессий движка — на место.
pub fn shutdown() {
    if let Some(old) = RUNNING.lock().take() {
        old.stop.store(true, Ordering::SeqCst);
        let _ = old.thread.join();
    }
}

/* ── Пересылка ─────────────────────────────────────────────────────────── */

fn relay(app: &AppHandle, stop: &AtomicBool, marker: &std::path::Path) {
    let _com = Com::init();
    let mut volumes = Volumes::new(marker);
    let mut warned = false;
    while !stop.load(Ordering::SeqCst) {
        let Some(engine) = engine_process() else {
            // Движок ещё не запущен (или перезапускается после падения).
            std::thread::sleep(Duration::from_millis(500));
            continue;
        };
        match Pipeline::open(engine) {
            Ok(mut pipeline) => {
                warned = false;
                if let Err(err) = pipeline.run(stop, &mut volumes) {
                    tracing::debug!(%err, "пересылка звука прервалась, открываю заново");
                }
            }
            Err(err) => {
                tracing::warn!(%err, "пересылка звука не открылась");
                if !warned {
                    warned = true;
                    let _ = app.emit(
                        "stream-audio",
                        serde_json::json!({
                            "error": "Windows не отдала звук движка: нужна Windows 11"
                        }),
                    );
                }
                std::thread::sleep(Duration::from_secs(2));
            }
        }
        // Пока пересылки нет, движок звучит как обычно.
        volumes.restore();
    }
    volumes.restore();
}

/// Захват звука движка и вывод его из процесса браузера.
struct Pipeline {
    engine: u32,
    capture: IAudioClient,
    capture_client: IAudioCaptureClient,
    event: Event,
    render: Option<Output>,
    backlog: VecDeque<f32>,
    /// Усиление приглушённого звука. Если захват идёт до громкости микшера
    /// (поведение Windows могло смениться), усиливать нечего.
    gain: f32,
    /// Сколько подряд звук приходил громким, хотя сессии приглушены.
    loud: Duration,
}

struct Output {
    device: String,
    client: IAudioClient,
    render: IAudioRenderClient,
    frames: u32,
}

impl Pipeline {
    fn open(engine: u32) -> windows::core::Result<Self> {
        let format = float_format();
        let capture = activate_process_loopback(engine)?;
        unsafe {
            capture.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK
                    | AUDCLNT_STREAMFLAGS_EVENTCALLBACK
                    | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
                CAPTURE_BUFFER,
                0,
                &format,
                None,
            )?;
        }
        let event = Event::new()?;
        unsafe { capture.SetEventHandle(event.0)? };
        let capture_client: IAudioCaptureClient = unsafe { capture.GetService()? };
        let render = Output::open().ok();
        unsafe { capture.Start()? };
        Ok(Self {
            engine,
            capture,
            capture_client,
            event,
            render,
            backlog: VecDeque::with_capacity(BACKLOG_FRAMES * usize::from(CHANNELS)),
            gain: 1.0 / QUIET,
            loud: Duration::ZERO,
        })
    }

    fn run(&mut self, stop: &AtomicBool, volumes: &mut Volumes) -> anyhow::Result<()> {
        let mut checked: Option<Instant> = None;
        // Список процессов снимается реже, чем ищутся новые сессии: сессия
        // появляется, когда страница заиграла, а процессы меняются редко.
        let mut surveyed: Option<Instant> = None;
        let mut tree = HashSet::new();
        while !stop.load(Ordering::SeqCst) {
            if checked.is_none_or(|at| at.elapsed() >= HOUSEKEEPING) {
                checked = Some(Instant::now());
                if surveyed.is_none_or(|at| at.elapsed() >= PROCESSES) {
                    surveyed = Some(Instant::now());
                    if engine_process() != Some(self.engine) {
                        anyhow::bail!("процесс движка сменился");
                    }
                    tree = process_tree(self.engine);
                    self.follow_default_device();
                }
                // Приглушать движок можно, только пока пересылке есть куда
                // играть: иначе страницы просто замолчали бы.
                if self.render.is_some() {
                    volumes.lower(&tree);
                } else {
                    volumes.restore();
                }
            }
            let woke = unsafe { WaitForSingleObject(self.event.0, 50) };
            if woke == WAIT_OBJECT_0 {
                self.drain(volumes.any_lowered())?;
            }
            self.feed();
        }
        Ok(())
    }

    /// Забрать всё, что снял захват.
    fn drain(&mut self, lowered: bool) -> windows::core::Result<()> {
        let channels = usize::from(CHANNELS);
        loop {
            let next = unsafe { self.capture_client.GetNextPacketSize()? };
            if next == 0 {
                return Ok(());
            }
            let mut data = std::ptr::null_mut();
            let mut frames = 0u32;
            let mut flags = 0u32;
            unsafe {
                self.capture_client
                    .GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
            }
            let samples = frames as usize * channels;
            let silent = flags & (AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0 || data.is_null();
            if silent {
                self.backlog.extend(std::iter::repeat_n(0.0, samples));
            } else {
                let packet = unsafe { std::slice::from_raw_parts(data.cast::<f32>(), samples) };
                let peak = packet
                    .iter()
                    .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
                self.calibrate(peak, lowered, frames);
                // Усиление, после которого звук упёрся бы в потолок, значит,
                // что этот кусок снят ещё до приглушения: он идёт как есть.
                let gain = if lowered && peak * self.gain <= 1.0 {
                    self.gain
                } else {
                    1.0
                };
                self.backlog
                    .extend(packet.iter().map(|sample| (sample * gain).clamp(-1.0, 1.0)));
            }
            unsafe { self.capture_client.ReleaseBuffer(frames)? };
            let limit = BACKLOG_FRAMES * channels;
            if self.backlog.len() > limit {
                let extra = self.backlog.len() - limit;
                self.backlog.drain(..extra);
            }
        }
    }

    /// Громкий звук при приглушённых сессиях дольше секунды — захват берёт
    /// поток до громкости микшера, и усиливать его не нужно.
    fn calibrate(&mut self, peak: f32, lowered: bool, frames: u32) {
        if !lowered || self.gain == 1.0 {
            return;
        }
        if peak > LOWERED * 2.0 {
            self.loud += Duration::from_secs_f64(f64::from(frames) / f64::from(RATE));
            if self.loud > Duration::from_secs(1) {
                tracing::info!("захват звука идёт до громкости микшера — без усиления");
                self.gain = 1.0;
            }
        } else if peak > 0.0 {
            self.loud = Duration::ZERO;
        }
    }

    /// Отдать накопленное устройству вывода — сколько оно сейчас примет.
    fn feed(&mut self) {
        let Some(output) = &self.render else {
            self.backlog.clear();
            return;
        };
        if let Err(err) = output.write(&mut self.backlog) {
            tracing::debug!(%err, "устройство вывода пропало");
            self.render = None;
        }
    }

    /// Устройство вывода по умолчанию сменилось (наушники, колонки) — звук
    /// уходит туда же, куда звучит всё остальное.
    fn follow_default_device(&mut self) {
        let current = default_device_id();
        let same =
            matches!((&self.render, &current), (Some(output), Some(id)) if output.device == *id);
        if same || (self.render.is_none() && current.is_none()) {
            return;
        }
        if let Some(output) = self.render.take() {
            let _ = unsafe { output.client.Stop() };
        }
        self.render = Output::open().ok();
    }
}

impl Drop for Pipeline {
    fn drop(&mut self) {
        unsafe {
            let _ = self.capture.Stop();
            if let Some(output) = &self.render {
                let _ = output.client.Stop();
            }
        }
    }
}

impl Output {
    /// Отдать устройству из `backlog` столько, сколько оно сейчас примет.
    fn write(&self, backlog: &mut VecDeque<f32>) -> windows::core::Result<()> {
        let channels = usize::from(CHANNELS);
        let padding = unsafe { self.client.GetCurrentPadding()? };
        let free = self.frames.saturating_sub(padding) as usize;
        let frames = free.min(backlog.len() / channels);
        if frames == 0 {
            return Ok(());
        }
        let buffer = unsafe { self.render.GetBuffer(frames as u32)? };
        let target =
            unsafe { std::slice::from_raw_parts_mut(buffer.cast::<f32>(), frames * channels) };
        for (slot, sample) in target.iter_mut().zip(backlog.drain(..frames * channels)) {
            *slot = sample;
        }
        unsafe { self.render.ReleaseBuffer(frames as u32, 0) }
    }

    fn open() -> windows::core::Result<Self> {
        let format = float_format();
        unsafe {
            let devices: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let device = devices.GetDefaultAudioEndpoint(eRender, eConsole)?;
            let id = device.GetId()?;
            let name = id.to_string().unwrap_or_default();
            CoTaskMemFree(Some(id.0 as *const _));
            let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                RENDER_BUFFER,
                0,
                &format,
                None,
            )?;
            let frames = client.GetBufferSize()?;
            let render: IAudioRenderClient = client.GetService()?;
            client.Start()?;
            Ok(Self {
                device: name,
                client,
                render,
                frames,
            })
        }
    }
}

fn default_device_id() -> Option<String> {
    unsafe {
        let devices: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        let device = devices.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
        let id = device.GetId().ok()?;
        let name = id.to_string().ok();
        CoTaskMemFree(Some(id.0 as *const _));
        name
    }
}

fn float_format() -> WAVEFORMATEX {
    let block = CHANNELS * 4;
    WAVEFORMATEX {
        wFormatTag: WAVE_FORMAT_IEEE_FLOAT,
        nChannels: CHANNELS,
        nSamplesPerSec: RATE,
        nAvgBytesPerSec: RATE * u32::from(block),
        nBlockAlign: block,
        wBitsPerSample: 32,
        cbSize: 0,
    }
}

/* ── Захват звука процесса ─────────────────────────────────────────────── */

#[implement(IActivateAudioInterfaceCompletionHandler)]
struct Activated(std::sync::mpsc::Sender<()>);

impl IActivateAudioInterfaceCompletionHandler_Impl for Activated_Impl {
    fn ActivateCompleted(
        &self,
        _operation: windows::core::Ref<'_, IActivateAudioInterfaceAsyncOperation>,
    ) -> windows::core::Result<()> {
        let _ = self.0.send(());
        Ok(())
    }
}

/// Аудиоклиент, который слышит процесс `pid` и все его дочерние.
fn activate_process_loopback(pid: u32) -> windows::core::Result<IAudioClient> {
    let mut params = AUDIOCLIENT_ACTIVATION_PARAMS {
        ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
        Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
            ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                TargetProcessId: pid,
                ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
            },
        },
    };
    // PROPVARIANT только ссылается на параметры: он не владеет ими и не
    // освобождается (PropVariantClear освободил бы чужую память).
    let mut prop = std::mem::ManuallyDrop::new(PROPVARIANT::default());
    unsafe {
        let inner = &mut *prop.Anonymous.Anonymous;
        inner.vt = VT_BLOB;
        inner.Anonymous.blob = BLOB {
            cbSize: std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
            pBlobData: (&mut params as *mut AUDIOCLIENT_ACTIVATION_PARAMS).cast(),
        };
    }
    let (done, answered) = std::sync::mpsc::channel();
    let handler: IActivateAudioInterfaceCompletionHandler = Activated(done).into();
    let operation = unsafe {
        ActivateAudioInterfaceAsync(
            VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
            &IAudioClient::IID,
            Some(&*prop as *const PROPVARIANT),
            &handler,
        )?
    };
    if answered.recv_timeout(Duration::from_secs(5)).is_err() {
        return Err(windows::core::Error::new(
            windows::Win32::Foundation::E_FAIL,
            "Windows не ответила на захват звука",
        ));
    }
    let mut result = windows::core::HRESULT(0);
    let mut activated: Option<windows::core::IUnknown> = None;
    unsafe { operation.GetActivateResult(&mut result, &mut activated)? };
    result.ok()?;
    activated
        .ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_POINTER))?
        .cast()
}

/* ── Громкость сессий движка ───────────────────────────────────────────── */

/// Сессии движка, которые браузер приглушил, и их прежняя громкость.
struct Volumes {
    marker: std::path::PathBuf,
    lowered: HashMap<String, (ISimpleAudioVolume, f32)>,
}

impl Volumes {
    fn new(marker: &std::path::Path) -> Self {
        Self {
            marker: marker.to_path_buf(),
            lowered: HashMap::new(),
        }
    }

    fn any_lowered(&self) -> bool {
        !self.lowered.is_empty()
    }

    /// Приглушить новые сессии движка. Сессия появляется, когда страница
    /// начинает играть, — проверяется часто.
    fn lower(&mut self, tree: &HashSet<u32>) {
        for (id, volume) in engine_sessions(tree) {
            if self.lowered.contains_key(&id) {
                continue;
            }
            let Ok(level) = (unsafe { volume.GetMasterVolume() }) else {
                continue;
            };
            if self.lowered.is_empty() {
                let _ = std::fs::write(&self.marker, b"1");
            }
            // Уже приглушена — остаток аварийного выхода: вернётся полной.
            let original = if level < LOWERED { 1.0 } else { level };
            if unsafe { volume.SetMasterVolume(QUIET, &GUID::zeroed()) }.is_ok() {
                self.lowered.insert(id, (volume, original));
            }
        }
    }

    fn restore(&mut self) {
        if self.lowered.is_empty() {
            return;
        }
        for (_, (volume, original)) in self.lowered.drain() {
            let _ = unsafe { volume.SetMasterVolume(original, &GUID::zeroed()) };
        }
        let _ = std::fs::remove_file(&self.marker);
    }
}

impl Drop for Volumes {
    fn drop(&mut self) {
        self.restore();
    }
}

/// Прошлый запуск закрылся, не вернув громкость: сессии движка, которые ещё
/// приглушены, получают полную. Сессия появляется, только когда страница
/// играет звук, поэтому поиск идёт какое-то время.
fn repair(stop: &AtomicBool, marker: &std::path::Path) {
    let _com = Com::init();
    let started = Instant::now();
    let mut restored = HashSet::new();
    while !stop.load(Ordering::SeqCst) && started.elapsed() < REPAIR_FOR {
        if let Some(engine) = engine_process() {
            for (id, volume) in engine_sessions(&process_tree(engine)) {
                if restored.contains(&id) {
                    continue;
                }
                let level = unsafe { volume.GetMasterVolume() }.unwrap_or(1.0);
                if level < LOWERED {
                    let _ = unsafe { volume.SetMasterVolume(1.0, &GUID::zeroed()) };
                    tracing::info!("громкость звука страниц возвращена после аварийного выхода");
                }
                restored.insert(id);
            }
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    if !stop.load(Ordering::SeqCst) {
        let _ = std::fs::remove_file(marker);
    }
}

/// Звуковые сессии процессов движка на всех устройствах вывода: номер сессии
/// и её громкость.
fn engine_sessions(tree: &HashSet<u32>) -> Vec<(String, ISimpleAudioVolume)> {
    let mut found = Vec::new();
    unsafe {
        let Ok(devices) =
            CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL)
        else {
            return found;
        };
        let Ok(collection) = devices.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) else {
            return found;
        };
        let count = collection.GetCount().unwrap_or(0);
        for index in 0..count {
            let Ok(device) = collection.Item(index) else {
                continue;
            };
            let Ok(manager) = device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else {
                continue;
            };
            let Ok(sessions) = manager.GetSessionEnumerator() else {
                continue;
            };
            for number in 0..sessions.GetCount().unwrap_or(0) {
                let Ok(session) = sessions.GetSession(number) else {
                    continue;
                };
                let Ok(session) = session.cast::<IAudioSessionControl2>() else {
                    continue;
                };
                let Ok(pid) = session.GetProcessId() else {
                    continue;
                };
                if !tree.contains(&pid) {
                    continue;
                }
                let Ok(raw) = session.GetSessionInstanceIdentifier() else {
                    continue;
                };
                let id = raw.to_string().unwrap_or_default();
                CoTaskMemFree(Some(raw.0 as *const _));
                if let Ok(volume) = session.cast::<ISimpleAudioVolume>() {
                    found.push((id, volume));
                }
            }
        }
    }
    found
}

/* ── Процессы движка ───────────────────────────────────────────────────── */

/// Процесс движка WebView2 браузера: `msedgewebview2.exe`, запущенный самим
/// браузером. Его дерево — все процессы страниц и звука.
fn engine_process() -> Option<u32> {
    let own = std::process::id();
    snapshot()
        .into_iter()
        .find(|process| process.parent == own && process.webview)
        .map(|process| process.pid)
}

/// Процесс `root` и все его потомки.
fn process_tree(root: u32) -> HashSet<u32> {
    let processes = snapshot();
    let mut tree = HashSet::from([root]);
    loop {
        let before = tree.len();
        for process in &processes {
            if tree.contains(&process.parent) && process.pid != process.parent {
                tree.insert(process.pid);
            }
        }
        if tree.len() == before {
            return tree;
        }
    }
}

struct Process {
    pid: u32,
    parent: u32,
    webview: bool,
}

fn snapshot() -> Vec<Process> {
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    let mut processes = Vec::new();
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return processes;
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                let end = entry
                    .szExeFile
                    .iter()
                    .position(|&unit| unit == 0)
                    .unwrap_or(entry.szExeFile.len());
                let name = String::from_utf16_lossy(&entry.szExeFile[..end]);
                processes.push(Process {
                    pid: entry.th32ProcessID,
                    parent: entry.th32ParentProcessID,
                    webview: name.eq_ignore_ascii_case("msedgewebview2.exe"),
                });
                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snapshot);
    }
    processes
}

/* ── Мелочи ────────────────────────────────────────────────────────────── */

/// COM на потоке пересылки — свой, многопоточный.
struct Com(bool);

impl Com {
    fn init() -> Self {
        Self(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok())
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

struct Event(HANDLE);

impl Event {
    fn new() -> windows::core::Result<Self> {
        Ok(Self(unsafe {
            CreateEventW(None, false, false, PCWSTR::null())?
        }))
    }
}

impl Drop for Event {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.0) };
    }
}
