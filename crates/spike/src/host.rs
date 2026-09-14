//! Голый WebView2-хост: окно, Environment, вкладки. Без Tauri — чтобы цифры
//! относились к движку, а не к обвязке.

use std::sync::mpsc;

use webview2_com::Microsoft::Web::WebView2::Win32::{
    CreateCoreWebView2EnvironmentWithOptions, ICoreWebView2, ICoreWebView2Controller,
    ICoreWebView2Environment, ICoreWebView2EnvironmentOptions,
};
use webview2_com::{
    CoreWebView2EnvironmentOptions, CreateCoreWebView2ControllerCompletedHandler,
    CreateCoreWebView2EnvironmentCompletedHandler,
};
use windows::Win32::Foundation::{E_POINTER, E_UNEXPECTED, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, PeekMessageW, PostQuitMessage,
    RegisterClassW, ShowWindow, TranslateMessage, CW_USEDEFAULT, MSG, PM_REMOVE, SW_SHOW,
    WM_DESTROY, WNDCLASSW, WS_OVERLAPPEDWINDOW,
};
use windows_core::{h, Interface, HSTRING, PCWSTR};

/// Аргументы движка. Autoplay-policy трогаем осознанно: без него у видео на
/// вкладке, которую пользователь только что открыл, не стартует звук, и это
/// читается как «браузер сломан». Остальное — минимум, чтобы не расходиться с
/// поведением Edge на тех же сайтах.
pub const BROWSER_ARGS: &str = "--autoplay-policy=no-user-gesture-required";

pub struct Host {
    pub hwnd: HWND,
    pub env: ICoreWebView2Environment,
    pub views: Vec<View>,
}

pub struct View {
    /// Держит вкладку живой: при дропе контроллера WebView2 закрывает
    /// вебвью, и замер памяти внезапно меряет пустое окно. Читать это поле
    /// незачем — важно, что оно существует.
    #[allow(dead_code)]
    pub controller: ICoreWebView2Controller,
    pub core: ICoreWebView2,
}

impl Host {
    /// Окно + Environment. `user_data` намеренно свой: спайк не должен
    /// трогать профиль установленного браузера.
    pub fn create(user_data: &str) -> anyhow::Result<Self> {
        let hwnd = create_window()?;

        // Аргументы браузера — те же, что пойдут в продакшн-сборку. Environment
        // переиспользует браузерный процесс только при полном совпадении
        // (user data folder + аргументы), поэтому расхождение здесь сделало бы
        // замер памяти замером чужой конфигурации.
        let options = CoreWebView2EnvironmentOptions::default();
        unsafe {
            options.set_additional_browser_arguments(BROWSER_ARGS.to_string());
        }

        let (tx, rx) = mpsc::channel();
        let handler =
            CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(move |code, env| {
                let result = (|| {
                    code?;
                    env.ok_or_else(|| windows_core::Error::from(E_POINTER))
                })();
                tx.send(result)
                    .map_err(|_| windows_core::Error::from(E_UNEXPECTED))
            }));

        let user_data = HSTRING::from(user_data);
        unsafe {
            CreateCoreWebView2EnvironmentWithOptions(
                PCWSTR::null(),
                &user_data,
                &ICoreWebView2EnvironmentOptions::from(options),
                &handler,
            )?;
        }
        let env = webview2_com::wait_with_pump(rx)??;

        Ok(Self {
            hwnd,
            env,
            views: Vec::new(),
        })
    }

    /// Ещё одна вкладка в том же Environment.
    pub fn open(&mut self, url: &str, bounds: RECT) -> anyhow::Result<usize> {
        let (tx, rx) = mpsc::channel();
        let handler = CreateCoreWebView2ControllerCompletedHandler::create(Box::new(
            move |code, controller| {
                let result = (|| {
                    code?;
                    controller.ok_or_else(|| windows_core::Error::from(E_POINTER))
                })();
                tx.send(result)
                    .map_err(|_| windows_core::Error::from(E_UNEXPECTED))
            },
        ));

        unsafe { self.env.CreateCoreWebView2Controller(self.hwnd, &handler)? };
        let controller = webview2_com::wait_with_pump(rx)??;
        let core = unsafe { controller.CoreWebView2()? };

        unsafe {
            controller.SetBounds(bounds)?;
            // Все вкладки, кроме первой, скрыты — ровно как в браузере.
            // Видимость сильно влияет на память: у скрытой вкладки нет
            // композитора и растровых слоёв.
            controller.SetIsVisible(self.views.is_empty())?;
            core.Navigate(&HSTRING::from(url))?;
        }

        self.views.push(View { controller, core });
        Ok(self.views.len() - 1)
    }

    /// PID процессов этого Environment: браузерный, GPU, рендереры, утилиты.
    /// Именно их сумма — честный ответ на «сколько ест 20 вкладок».
    pub fn process_ids(&self) -> anyhow::Result<Vec<(u32, i32)>> {
        use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Environment8;

        let env8: ICoreWebView2Environment8 = self.env.cast()?;
        let mut out = Vec::new();
        unsafe {
            let collection = env8.GetProcessInfos()?;
            let mut count = 0u32;
            collection.Count(&mut count)?;
            for index in 0..count {
                let info = collection.GetValueAtIndex(index)?;
                let mut pid = 0i32;
                info.ProcessId(&mut pid)?;
                let mut kind = Default::default();
                info.Kind(&mut kind)?;
                out.push((pid as u32, kind.0));
            }
        }
        Ok(out)
    }

    pub fn show(&self) {
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOW);
        }
    }
}

/// Прокрутить очередь сообщений `millis` миллисекунд.
///
/// WebView2 доставляет всё — завершение навигации, коллбеки, работу
/// композитора — через оконные сообщения. `sleep` вместо насоса заморозил бы
/// загрузку страниц, и замер памяти получился бы на пустых вкладках.
pub fn pump(millis: u64) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(millis);
    let mut message = MSG::default();
    while std::time::Instant::now() < deadline {
        unsafe {
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(4));
    }
}

fn create_window() -> anyhow::Result<HWND> {
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let class_name = h!("Browser190x4SpikeWindow");

        let class = WNDCLASSW {
            hInstance: instance.into(),
            lpszClassName: PCWSTR(class_name.as_ptr()),
            lpfnWndProc: Some(wndproc),
            ..Default::default()
        };
        RegisterClassW(&class);

        let hwnd = CreateWindowExW(
            Default::default(),
            PCWSTR(class_name.as_ptr()),
            h!("190x4 spike"),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1600,
            1000,
            None,
            None,
            Some(instance.into()),
            None,
        )?;
        Ok(hwnd)
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_DESTROY {
            PostQuitMessage(0);
            return LRESULT(0);
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }
}
