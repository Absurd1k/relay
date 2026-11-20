#![allow(unsafe_op_in_unsafe_fn)]
use relay::{
    Settings,
    engine::{self, Action, Backend, Check, Event, Kind},
    platform,
};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::PathBuf,
    ptr::{null, null_mut},
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, IsWindowEnabled};
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_SETVERSION, NIN_SELECT,
    NOTIFYICON_VERSION_4, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::{DataExchange::*, LibraryLoader::GetModuleHandleW, Memory::*},
    UI::{Controls::*, WindowsAndMessaging::*},
};

const TRAY_MESSAGE: u32 = WM_APP + 2;
const SHOW_MESSAGE: u32 = WM_APP + 3;
// TBM_GETPOS is WM_USER in commctrl.h; windows-sys omits this zero-offset alias.
const TBM_GETPOS: u32 = WM_USER;

const fn rgb(r: u32, g: u32, b: u32) -> u32 {
    r | (g << 8) | (b << 16)
}
const BG: u32 = rgb(8, 19, 22);
const PANEL: u32 = rgb(10, 25, 24);
const CARD: u32 = rgb(17, 37, 32);
const LINE: u32 = rgb(40, 71, 62);
const TEXT: u32 = rgb(231, 237, 244);
const MUTED: u32 = rgb(153, 167, 185);
const ACCENT: u32 = rgb(111, 232, 193);
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
fn rect(x: i32, y: i32, w: i32, h: i32) -> RECT {
    RECT {
        left: x,
        top: y,
        right: x + w,
        bottom: y + h,
    }
}

struct App {
    hwnd: HWND,
    icon: HICON,
    hero_icon: HICON,
    backend: Option<Backend>,
    data: PathBuf,
    settings: Settings,
    page: usize,
    ready: bool,
    strategies: Vec<String>,
    running: [bool; 2],
    busy: [bool; 2],
    checks: Vec<Check>,
    checking: bool,
    logs: VecDeque<String>,
    message: String,
    error: bool,
    memory: f64,
    admin: bool,
    buttons: BTreeMap<u32, HWND>,
    visible: BTreeSet<u32>,
    port_edit: HWND,
    port_font: HFONT,
    log_offset: usize,
    ai_running: bool,
    ai_busy: bool,
    close_after_ai: bool,
    startup_busy: bool,
    tray_icon: HICON,
    tray_added: bool,
    taskbar_created: u32,
    quitting: bool,
    winter: crate::winter::Winter,
    surface: Option<crate::winter::Surface>,
    animation_timer: bool,
    update_status: String,
    update_ready: Option<relay::updates::Download>,
    update_busy: bool,
    install_pending: bool,
    close_after_service: bool,
    alt_busy: bool,
    diagnosing: bool,
    show_diagnostics: bool,
    findings: Vec<relay::diagnostics::Finding>,
    sliders: [HWND; 2],
}
impl App {
    fn new(icon: HICON, hero_icon: HICON, tray_icon: HICON) -> Self {
        let data = relay::data_dir();
        let loaded = Settings::load(&data.join("config.json"));
        let error = loaded.as_ref().err().cloned();
        Self {
            hwnd: null_mut(),
            icon,
            hero_icon,
            backend: None,
            data,
            settings: loaded.unwrap_or_default(),
            page: 0,
            ready: false,
            strategies: vec![],
            running: [false; 2],
            busy: [false; 2],
            checks: vec![],
            checking: false,
            logs: VecDeque::new(),
            message: error
                .clone()
                .unwrap_or_else(|| "Подготовка встроенных компонентов…".into()),
            error: error.is_some(),
            memory: 0.0,
            admin: platform::is_admin(),
            buttons: BTreeMap::new(),
            visible: BTreeSet::new(),
            port_edit: null_mut(),
            port_font: null_mut(),
            log_offset: 0,
            ai_running: false,
            ai_busy: false,
            close_after_ai: false,
            startup_busy: false,
            tray_icon,
            tray_added: false,
            taskbar_created: 0,
            quitting: false,
            winter: crate::winter::Winter::new(),
            surface: None,
            animation_timer: false,
            update_status: "Проверка GitHub только при запуске".into(),
            update_ready: None,
            update_busy: false,
            install_pending: false,
            close_after_service: false,
            alt_busy: false,
            diagnosing: false,
            show_diagnostics: false,
            findings: vec![],
            sliders: [null_mut(); 2],
        }
    }
    fn notify(&mut self, s: String, error: bool) {
        engine::push_log(&mut self.logs, s.clone());
        self.message = s;
        self.error = error;
    }
    fn send(&mut self, a: Action) -> bool {
        match self
            .backend
            .as_ref()
            .ok_or("Компоненты ещё не готовы".to_string())
            .and_then(|b| b.send(a))
        {
            Ok(()) => true,
            Err(e) => {
                self.notify(e, true);
                false
            }
        }
    }
    fn save(&mut self) -> bool {
        match self.settings.save(&self.data.join("config.json")) {
            Ok(()) => true,
            Err(e) => {
                self.notify(e, true);
                false
            }
        }
    }
    fn toggle(&mut self, i: usize) {
        if self.busy[i] {
            return;
        }
        let old = self.settings.clone();
        if i == 0 {
            self.settings.resume_telegram = !self.running[i];
        } else {
            self.settings.resume_zapret = !self.running[i];
        }
        if !self.save() {
            self.settings = old;
            return;
        }
        let kind = if i == 0 { Kind::Telegram } else { Kind::Zapret };
        let a = if self.running[i] {
            Action::Stop(kind)
        } else {
            Action::Start(kind, self.settings.clone())
        };
        if self.send(a) {
            self.busy[i] = true;
        }
    }
    fn restore_saved(&mut self) {
        let mut settings = self.settings.clone();
        if std::env::args().any(|a| a == "--resume-after-update") {
            settings.auto_connect = true;
        }
        let plan = settings.restore_plan();
        if plan.is_empty() {
            return;
        }
        if !self.admin && plan.iter().any(|k| *k != Kind::Telegram) {
            match platform::elevate() {
                Ok(()) => unsafe {
                    self.quitting = true;
                    PostMessageW(self.hwnd, WM_CLOSE, 0, 0);
                },
                Err(e) => self.notify(format!("Автоподключение: {e}"), true),
            }
            return;
        }
        for kind in plan {
            if self.send(Action::Start(kind, self.settings.clone())) {
                match kind {
                    Kind::Telegram => self.busy[0] = true,
                    Kind::Zapret => self.busy[1] = true,
                    Kind::Ai => self.ai_busy = true,
                }
            }
        }
    }
    fn drain(&mut self) -> bool {
        let mut changed = false;
        loop {
            let event = self.backend.as_ref().and_then(|b| b.rx.try_recv().ok());
            let Some(event) = event else { break };
            changed = true;
            match event {
                Event::Profile(result) => {
                    self.busy[1] = false;
                    match result {
                        Ok(settings) => {
                            self.settings = settings;
                            if self.save() {
                                self.notify(
                                    format!(
                                        "Включён {}",
                                        relay::strategy_label(&self.settings.strategy)
                                    ),
                                    false,
                                );
                            }
                        }
                        Err(e) => self.notify(e, true),
                    }
                }
                Event::Diagnostics(findings) => {
                    self.findings = findings;
                    self.diagnosing = false;
                    self.notify(
                        "Диагностика завершена. Системные настройки не изменялись.".into(),
                        false,
                    );
                }
                Event::AltProgress(message) => {
                    engine::push_log(&mut self.logs, message.clone());
                    self.notify(message, false);
                }
                Event::AltDone { result, running } => {
                    self.alt_busy = false;
                    self.running[1] = running;
                    self.busy[1] = false;
                    match result {
                        Ok(Some(profile)) => {
                            self.settings.strategy = profile;
                            self.settings.resume_zapret = running;
                            if self.save() {
                                self.notify(format!("{} прошёл HTTPS-проверки. Голос и видео проверьте в приложениях.",relay::strategy_label(&self.settings.strategy)),false);
                            }
                        }
                        Ok(None) => self.notify(
                            "Подбор отменён. Прежний профиль и подключение восстановлены.".into(),
                            false,
                        ),
                        Err(e) => self.notify(e, true),
                    }
                }
                Event::Ready(r) => match r {
                    Ok((_, s)) => {
                        self.strategies = s;
                        self.ready = true;
                        if !self.strategies.contains(&self.settings.strategy) {
                            self.settings.strategy = "general (ALT).bat".into();
                        }
                        self.notify("Компоненты готовы. Можно подключаться.".into(), false);
                        self.restore_saved();
                        self.send(Action::AutoUpdates(self.settings.auto_update));
                    }
                    Err(e) => self.notify(e, true),
                },
                Event::State(k, r, m) => {
                    if k == Kind::Ai {
                        self.ai_running = r;
                        self.ai_busy = false;
                        if self.close_after_ai {
                            self.close_after_ai = false;
                            if !r {
                                unsafe {
                                    PostMessageW(self.hwnd, WM_CLOSE, 0, 0);
                                }
                            } else {
                                self.quitting = false;
                                self.install_pending = false;
                                unsafe {
                                    self.show_window();
                                }
                            }
                        }
                    } else {
                        let i = if k == Kind::Telegram { 0 } else { 1 };
                        self.running[i] = r;
                        self.busy[i] = false;
                        if k == Kind::Zapret && self.close_after_service {
                            self.close_after_service = false;
                            if !r {
                                unsafe {
                                    PostMessageW(self.hwnd, WM_CLOSE, 0, 0);
                                }
                            } else {
                                self.quitting = false;
                                self.install_pending = false;
                                unsafe {
                                    self.show_window();
                                }
                            }
                        }
                    }
                    let error = (!r && m != "Остановлен") || m.starts_with("Не удалось");
                    self.notify(format!("{}: {m}", k.name()), error);
                    self.checks.clear();
                }
                Event::Log(s) => engine::push_log(&mut self.logs, s),
                Event::Checks(c) => {
                    self.checks = c;
                    self.checking = false;
                    self.notify(
                        "HTTPS-проверка завершена. Результаты — в разделе «Проверка сети».".into(),
                        false,
                    );
                }
                Event::Memory(m) => self.memory = m,
                Event::Startup(result) => {
                    self.startup_busy = false;
                    match result {
                        Ok(settings) => {
                            self.settings = settings;
                            self.notify(if self.settings.auto_start{"Автозапуск включён. Relay восстановит сохранённые сервисы при входе в Windows."}else{"Автозапуск отключён."}.into(),false);
                        }
                        Err(e) => self.notify(e, true),
                    }
                }
                Event::Update(status) => {
                    use relay::updates::Status;
                    match status {
                        Status::Checking => {
                            self.update_busy = true;
                            self.update_status = "Проверка новых релизов GitHub…".into();
                        }
                        Status::Downloading {
                            version,
                            bytes,
                            total,
                        } => {
                            self.update_busy = true;
                            self.update_status =
                                format!("Скачивается v{version} · {}%", bytes * 100 / total.max(1));
                        }
                        Status::Ready(download) => {
                            self.update_busy = false;
                            self.update_status =
                                format!("Версия {} скачана и проверена", download.version);
                            let announce = self
                                .update_ready
                                .as_ref()
                                .is_none_or(|d| d.version != download.version);
                            self.update_ready = Some(download);
                            if announce {
                                unsafe {
                                    self.update_balloon();
                                }
                            }
                        }
                        Status::Current => {
                            self.update_busy = false;
                            self.update_status = format!(
                                "Установлена актуальная версия {}",
                                env!("CARGO_PKG_VERSION")
                            );
                        }
                        Status::Skipped => {
                            self.update_busy = false;
                        }
                        Status::Error(e) => {
                            self.update_busy = false;
                            self.update_status = format!("Не удалось проверить обновление: {e}");
                            engine::push_log(&mut self.logs, self.update_status.clone());
                        }
                    }
                }
            }
        }
        changed
    }
    unsafe fn action(&mut self, id: u32) {
        if self.alt_busy && matches!(id,1|10..=11|13|16|20..=21|60..=61|71|83|90|2000..=2999) {
            self.notify("Дождитесь подбора ALT или нажмите «Отменить».".into(), true);
            return;
        }
        match id {
            100..=107 => {
                self.page = (id - 100) as usize;
                ShowWindow(
                    self.port_edit,
                    if self.page == 3 { SW_SHOW } else { SW_HIDE },
                );
                for slider in self.sliders {
                    ShowWindow(slider, if self.page == 6 { SW_SHOW } else { SW_HIDE });
                }
            }
            2000..=2999 => {
                if let Some(profile) = self.strategies.get((id - 2000) as usize).cloned() {
                    self.apply_profile(profile);
                }
            }
            90 => {
                if !self.ready || self.busy.iter().any(|b| *b) || self.startup_busy {
                    return;
                }
                if !self.admin {
                    self.notify("Подбор ALT требует прав администратора.".into(), true);
                    return;
                }
                if let Some(backend) = &self.backend {
                    backend
                        .cancel_alt
                        .store(false, std::sync::atomic::Ordering::Relaxed);
                }
                if self.send(Action::AutoAlt(self.settings.clone())) {
                    self.alt_busy = true;
                    self.busy[1] = true;
                    self.show_diagnostics = false;
                    self.notify(
                        "Подбор ALT: соединение Discord / YouTube может кратко прерываться.".into(),
                        false,
                    );
                }
            }
            91 => {
                if let Some(backend) = &self.backend {
                    backend
                        .cancel_alt
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                }
                self.notify("Отмена после завершения текущей проверки…".into(), false);
            }
            92 => {
                self.page = 1;
                self.show_diagnostics = true;
                self.show_window();
                for slider in self.sliders {
                    ShowWindow(slider, SW_HIDE);
                }
                ShowWindow(self.port_edit, SW_HIDE);
                if !self.diagnosing
                    && self.send(Action::Diagnose(self.settings.port, self.running[0]))
                {
                    self.diagnosing = true;
                }
            }
            1 => {
                if self.running.iter().any(|b| *b) {
                    for i in 0..2 {
                        if self.running[i] {
                            self.toggle(i);
                        }
                    }
                    if self.ai_running && !self.ai_busy {
                        self.settings.resume_ai = false;
                        if self.save() && self.send(Action::Stop(Kind::Ai)) {
                            self.ai_busy = true;
                        }
                    }
                } else {
                    if self.settings.telegram {
                        self.toggle(0);
                    }
                    if self.settings.zapret {
                        self.toggle(1);
                    }
                }
            }
            10 => self.toggle(0),
            11 => self.toggle(1),
            12 => {
                if let Err(e) = platform::open_telegram(&self.settings.link()) {
                    self.notify(e, true)
                }
            }
            13 => {
                if self.running.iter().any(|b| *b)
                    || self.busy.iter().any(|b| *b)
                    || self.ai_running
                    || self.ai_busy
                {
                    self.notify("Сначала отключите движки перед перезапуском.".into(), true);
                } else {
                    match platform::elevate() {
                        Ok(()) => {
                            self.quitting = true;
                            PostMessageW(self.hwnd, WM_CLOSE, 0, 0);
                        }
                        Err(e) => self.notify(e, true),
                    }
                }
            }
            14 => {
                self.show_diagnostics = false;
                if !self.checking && !self.alt_busy && self.send(Action::Check) {
                    self.checking = true;
                }
            }
            15 => {
                self.settings.telegram = !self.settings.telegram;
                if !self.settings.telegram {
                    self.settings.resume_telegram = false;
                }
                self.save();
            }
            16 => {
                self.settings.zapret = !self.settings.zapret;
                if !self.settings.zapret {
                    self.settings.resume_zapret = false;
                }
                self.save();
            }
            20 => {
                let menu = CreatePopupMenu();
                for (i, n) in self.strategies.iter().enumerate() {
                    AppendMenuW(
                        menu,
                        MF_STRING,
                        1000 + i,
                        wide(&relay::strategy_label(n)).as_ptr(),
                    );
                }
                let mut p = POINT::default();
                GetCursorPos(&mut p);
                let chosen = TrackPopupMenu(
                    menu,
                    TPM_RETURNCMD | TPM_LEFTALIGN,
                    p.x,
                    p.y,
                    0,
                    self.hwnd,
                    null(),
                );
                DestroyMenu(menu);
                if chosen >= 1000
                    && let Some(n) = self.strategies.get(chosen as usize - 1000)
                {
                    self.settings.strategy = n.clone();
                    self.save();
                }
            }
            21 => {
                let mut buf = [0u16; 16];
                let n = GetWindowTextW(self.port_edit, buf.as_mut_ptr(), 16);
                match String::from_utf16_lossy(&buf[..n as usize]).parse::<u16>() {
                    Ok(port) if port >= 1024 => {
                        self.settings.port = port;
                        if self.save() {
                            self.notify("Настройки сохранены.".into(), false);
                        }
                    }
                    _ => self.notify("Введите порт от 1024 до 65535.".into(), true),
                }
            }
            22 => match clipboard(self.hwnd, &self.settings.link()) {
                Ok(()) => self.notify(
                    "Ссылка скопирована. Она содержит секрет прокси — не публикуйте её.".into(),
                    false,
                ),
                Err(e) => self.notify(e, true),
            },
            30 => {
                let s = self.logs.iter().cloned().collect::<Vec<_>>().join("\r\n");
                if let Err(e) = clipboard(self.hwnd, &s) {
                    self.notify(e, true)
                }
            }
            31 => {
                self.logs.clear();
                self.log_offset = 0;
            }
            50 => {
                if !self.ai_busy {
                    let old = self.settings.resume_ai;
                    self.settings.resume_ai = !self.ai_running;
                    if !self.save() {
                        self.settings.resume_ai = old;
                        return;
                    }
                    let action = if self.ai_running {
                        Action::Stop(Kind::Ai)
                    } else {
                        Action::Start(Kind::Ai, self.settings.clone())
                    };
                    if self.send(action) {
                        self.ai_busy = true;
                    }
                }
            }
            60 => {
                if !self.save() {
                    return;
                }
                let mut settings = self.settings.clone();
                settings.auto_start = !settings.auto_start;
                if settings.auto_start {
                    settings.auto_connect = true;
                }
                if self.send(Action::Startup(settings)) {
                    self.startup_busy = true;
                    self.notify("Обновление автозапуска Windows…".into(), false);
                }
            }
            61 => {
                let old = self.settings.clone();
                self.settings.auto_connect = !self.settings.auto_connect;
                if self.settings.auto_connect {
                    self.settings.resume_telegram = self.running[0];
                    self.settings.resume_zapret = self.running[1];
                    self.settings.resume_ai = self.ai_running;
                }
                if !self.save() {
                    self.settings = old;
                }
            }
            62 => {
                self.settings.winter_animation = !self.settings.winter_animation;
                self.save();
                self.refresh_animation();
            }
            80 => {
                self.settings.auto_update = !self.settings.auto_update;
                self.save();
                self.notify(
                    "Настройка сохранена для следующего запуска. Ручная проверка доступна ниже."
                        .into(),
                    false,
                );
            }
            81 => {
                if !self.update_busy {
                    self.update_busy = true;
                    self.send(Action::CheckUpdate);
                }
            }
            82 => {
                self.page = 5;
                self.show_window();
            }
            83 => {
                if let Some(download) = &self.update_ready {
                    if let Err(e) = relay::updates::verify_download(download) {
                        self.notify(e, true);
                        return;
                    }
                    self.settings.resume_telegram = self.running[0];
                    self.settings.resume_zapret = self.running[1];
                    self.settings.resume_ai = self.ai_running;
                    if !self.save() {
                        return;
                    }
                    self.install_pending = true;
                    self.quitting = true;
                    PostMessageW(self.hwnd, WM_CLOSE, 0, 0);
                }
            }
            44 => self.page = 4,
            40 => open_url("https://github.com/valnesfjord/tg-ws-proxy-rs"),
            41 => open_url("https://github.com/Flowseal/zapret-discord-youtube"),
            42 => open_url("https://info.dns.malw.link"),
            45 => open_url("https://chatgpt.com"),
            46 => open_url("https://claude.ai"),
            47 => open_url("https://gemini.google.com"),
            43 => open_url("https://github.com/UglyRacoon/eurodns"),
            70 => self.show_window(),
            71 => {
                self.quitting = true;
                PostMessageW(self.hwnd, WM_CLOSE, 0, 0);
            }
            _ => {}
        }
        InvalidateRect(self.hwnd, null(), 0);
        for button in self.buttons.values() {
            InvalidateRect(*button, null(), 0);
        }
    }
    unsafe fn apply_profile(&mut self, profile: String) {
        if self.busy[1] || self.alt_busy {
            return;
        }
        let mut next = self.settings.clone();
        next.strategy = profile;
        if self.running[1] {
            if self.send(Action::SwitchAlt(self.settings.clone(), next)) {
                self.busy[1] = true;
            }
        } else {
            self.settings = next;
            self.save();
        }
    }
    unsafe fn refresh_animation(&mut self) {
        let mut allowed: i32 = 1;
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            &mut allowed as *mut _ as _,
            0,
        );
        let on = self.settings.winter_animation
            && allowed != 0
            && IsWindowVisible(self.hwnd) != 0
            && GetForegroundWindow() == self.hwnd
            && !self.quitting;
        self.winter.running(on);
        if on && !self.animation_timer {
            SetTimer(
                self.hwnd,
                2,
                1000 / u32::from(self.settings.animation_fps.clamp(15, 60)),
                None,
            );
            self.animation_timer = true;
        } else if on {
            SetTimer(
                self.hwnd,
                2,
                1000 / u32::from(self.settings.animation_fps.clamp(15, 60)),
                None,
            );
        } else if !on && self.animation_timer {
            KillTimer(self.hwnd, 2);
            self.animation_timer = false;
        }
        if IsWindowVisible(self.hwnd) == 0 {
            self.surface = None;
        }
    }
    unsafe fn update_balloon(&self) {
        if !self.tray_added {
            return;
        }
        use windows_sys::Win32::UI::Shell::{NIF_INFO, NIIF_INFO, NIM_MODIFY};
        let mut data: NOTIFYICONDATAW = std::mem::zeroed();
        data.cbSize = std::mem::size_of_val(&data) as u32;
        data.hWnd = self.hwnd;
        data.uID = 1;
        data.uFlags = NIF_INFO;
        data.dwInfoFlags = NIIF_INFO;
        for (dst, c) in data
            .szInfoTitle
            .iter_mut()
            .zip("Relay — обновление скачано".encode_utf16())
        {
            *dst = c;
        }
        for(dst,c)in data.szInfo.iter_mut().zip("Откройте раздел «Обновления», чтобы перезапустить приложение и установить новую версию.".encode_utf16()){*dst=c;}
        Shell_NotifyIconW(NIM_MODIFY, &data);
    }
    unsafe fn show_window(&self) {
        ShowWindow(
            self.hwnd,
            if IsIconic(self.hwnd) != 0 {
                SW_RESTORE
            } else {
                SW_SHOW
            },
        );
        SetForegroundWindow(self.hwnd);
    }
    unsafe fn add_tray(&mut self) -> bool {
        let mut data: NOTIFYICONDATAW = std::mem::zeroed();
        data.cbSize = std::mem::size_of_val(&data) as u32;
        data.hWnd = self.hwnd;
        data.uID = 1;
        data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
        data.uCallbackMessage = TRAY_MESSAGE;
        data.hIcon = self.tray_icon;
        for (dst, value) in data
            .szTip
            .iter_mut()
            .zip("Relay — открыть окно / правый клик для выхода".encode_utf16())
        {
            *dst = value;
        }
        if Shell_NotifyIconW(NIM_ADD, &data) == 0 {
            return false;
        }
        data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        Shell_NotifyIconW(NIM_SETVERSION, &data);
        true
    }
    unsafe fn remove_tray(&mut self) {
        if !self.tray_added {
            return;
        }
        let mut data: NOTIFYICONDATAW = std::mem::zeroed();
        data.cbSize = std::mem::size_of_val(&data) as u32;
        data.hWnd = self.hwnd;
        data.uID = 1;
        Shell_NotifyIconW(NIM_DELETE, &data);
        self.tray_added = false;
    }
    unsafe fn tray_menu(&mut self) {
        let menu = CreatePopupMenu();
        AppendMenuW(menu, MF_STRING, 70, wide("Открыть Relay").as_ptr());
        for (id, label, active, busy) in [
            (10, "Telegram", self.running[0], self.busy[0]),
            (
                11,
                "Discord / YouTube",
                self.running[1],
                self.busy[1] || self.alt_busy,
            ),
            (50, "AI-маршрут", self.ai_running, self.ai_busy),
        ] {
            AppendMenuW(
                menu,
                MF_STRING
                    | if active { MF_CHECKED } else { 0 }
                    | if busy || !self.ready { MF_GRAYED } else { 0 },
                id,
                wide(label).as_ptr(),
            );
        }
        let profiles = CreatePopupMenu();
        for (i, profile) in self.strategies.iter().enumerate() {
            AppendMenuW(
                profiles,
                MF_STRING
                    | if *profile == self.settings.strategy {
                        MF_CHECKED
                    } else {
                        0
                    },
                2000 + i,
                wide(&relay::strategy_label(profile)).as_ptr(),
            );
        }
        AppendMenuW(
            menu,
            MF_POPUP
                | if self.alt_busy || self.busy[1] {
                    MF_GRAYED
                } else {
                    0
                },
            profiles as usize,
            wide("Профиль ALT").as_ptr(),
        );
        AppendMenuW(
            menu,
            MF_STRING
                | if self.alt_busy || !self.admin {
                    MF_GRAYED
                } else {
                    0
                },
            90,
            wide("Подобрать ALT").as_ptr(),
        );
        AppendMenuW(menu, MF_STRING, 92, wide("Диагностика конфликтов").as_ptr());
        AppendMenuW(
            menu,
            MF_STRING | if self.update_busy { MF_GRAYED } else { 0 },
            81,
            wide("Проверить обновления").as_ptr(),
        );
        if self.update_ready.is_some() {
            AppendMenuW(menu, MF_STRING, 82, wide("Обновление готово…").as_ptr());
        }
        AppendMenuW(menu, MF_SEPARATOR, 0, null());
        AppendMenuW(menu, MF_STRING, 71, wide("Выход").as_ptr());
        let mut point = POINT::default();
        GetCursorPos(&mut point);
        SetForegroundWindow(self.hwnd);
        let chosen = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            point.x,
            point.y,
            0,
            self.hwnd,
            null(),
        );
        DestroyMenu(menu);
        PostMessageW(self.hwnd, WM_NULL, 0, 0);
        if chosen != 0 {
            self.action(chosen as u32);
        }
    }
    unsafe fn button(&mut self, id: u32, r: RECT, label: &str, enabled: bool) {
        let enabled = enabled
            && (!self.startup_busy || !matches!(id,1|10..=11|13|15..=16|20..=21|50|60..=61));
        let enabled = enabled && !self.install_pending;
        let enabled =
            enabled && !(self.alt_busy && matches!(id,1|10..=11|13|16|20..=21|60..=61|71|83|90));
        self.visible.insert(id);
        let hwnd = *self.buttons.entry(id).or_insert_with(|| {
            let hwnd = CreateWindowExW(
                0,
                wide("BUTTON").as_ptr(),
                null(),
                WS_CHILD | WS_TABSTOP | BS_OWNERDRAW as u32,
                r.left,
                r.top,
                r.right - r.left,
                r.bottom - r.top,
                self.hwnd,
                id as usize as HMENU,
                GetModuleHandleW(null()),
                null(),
            );
            round_control(hwnd, r.right - r.left, r.bottom - r.top);
            hwnd
        });
        let mut old = RECT::default();
        GetWindowRect(hwnd, &mut old);
        let mut pos = POINT {
            x: old.left,
            y: old.top,
        };
        ScreenToClient(self.hwnd, &mut pos);
        if pos.x != r.left
            || pos.y != r.top
            || old.right - old.left != r.right - r.left
            || old.bottom - old.top != r.bottom - r.top
        {
            MoveWindow(hwnd, r.left, r.top, r.right - r.left, r.bottom - r.top, 1);
            round_control(hwnd, r.right - r.left, r.bottom - r.top);
        }
        let mut old_text = [0u16; 256];
        let n = GetWindowTextW(hwnd, old_text.as_mut_ptr(), 256);
        if String::from_utf16_lossy(&old_text[..n as usize]) != label {
            SetWindowTextW(hwnd, wide(label).as_ptr());
        }
        if (IsWindowEnabled(hwnd) != 0) != enabled {
            EnableWindow(hwnd, if enabled { 1 } else { 0 });
        }
        if IsWindowVisible(hwnd) == 0 {
            ShowWindow(hwnd, SW_SHOW);
        }
    }
    unsafe fn draw(&mut self, dc: HDC, w: i32, h: i32) {
        for slider in self.sliders {
            if (IsWindowVisible(slider) != 0) != (self.page == 6) {
                ShowWindow(slider, if self.page == 6 { SW_SHOW } else { SW_HIDE });
            }
        }
        if (IsWindowVisible(self.port_edit) != 0) != (self.page == 3) {
            ShowWindow(
                self.port_edit,
                if self.page == 3 { SW_SHOW } else { SW_HIDE },
            );
        }
        self.visible.clear();
        self.winter.draw(dc, w, h, self.settings.snow_count);
        fill(dc, rect(0, 0, 204, h), PANEL);
        fill(dc, rect(204, 0, 1, h), LINE);
        DrawIconEx(dc, 27, 32, self.icon, 32, 32, 0, null_mut(), DI_NORMAL);
        text(dc, "relay", rect(73, 24, 120, 46), 32, TEXT, DT_SINGLELINE);
        text(
            dc,
            "WINTER EDITION / 2027",
            rect(24, 85, 172, 20),
            10,
            MUTED,
            DT_SINGLELINE,
        );
        for (i, label) in [
            "Подключение",
            "Проверка сети",
            "Журнал",
            "Настройки",
            "Нейросети",
            "Обновления",
            "Оформление",
            "О приложении",
        ]
        .iter()
        .enumerate()
        {
            self.button(
                100 + i as u32,
                rect(20, 145 + i as i32 * 49, 164, 40),
                label,
                true,
            );
        }
        if h >= 760 {
            crate::winter::Winter::tree(dc, 102, 540);
            text(
                dc,
                "Тёплых праздников",
                rect(20, 620, 164, 24),
                13,
                rgb(217, 180, 102),
                DT_CENTER | DT_SINGLELINE,
            );
        }
        text(
            dc,
            "Portable · Rust / Win32",
            rect(24, h - 108, 173, 22),
            12,
            MUTED,
            DT_SINGLELINE,
        );
        text(
            dc,
            &if self.memory > 0.0 {
                format!("RAM · {:.0} МБ всего", self.memory)
            } else {
                "Нативный интерфейс".into()
            },
            rect(24, h - 80, 173, 22),
            12,
            ACCENT,
            DT_SINGLELINE,
        );
        text(
            dc,
            concat!("v", env!("CARGO_PKG_VERSION"), " · Windows x64"),
            rect(24, h - 50, 170, 22),
            11,
            MUTED,
            DT_SINGLELINE,
        );
        let x = 232;
        let width = w - x - 28;
        let (title, sub) = match self.page {
            0 => (
                "Всё на связи. Даже в метель.",
                "Зимний Relay · Telegram, Discord, YouTube и AI",
            ),
            1 => (
                "Проверка сети",
                "Реальные HTTPS-запросы с этого компьютера.",
            ),
            2 => ("Журнал", "Последние 300 событий. Секрет прокси скрывается."),
            3 => ("Настройки", "Только то, что нужно для подключения."),
            4 => (
                "Доступ к нейросетям",
                "Внешний адрес, который видит сервис.",
            ),
            5 => (
                "Обновления",
                "Новые релизы скачиваются автоматически из Absurd1k/relay.",
            ),
            6 => (
                "Оформление",
                "Настройте снег и плавность под свой компьютер.",
            ),
            _ => (
                concat!("Relay / ", env!("CARGO_PKG_VERSION")),
                "Нативная утилита, собранная с уважением к ресурсам ПК.",
            ),
        };
        text(dc, title, rect(x, 28, width, 42), 28, TEXT, DT_SINGLELINE);
        text(
            dc,
            sub,
            rect(x, 77, width - 230, 26),
            14,
            MUTED,
            DT_SINGLELINE | DT_END_ELLIPSIS,
        );
        self.button(
            62,
            rect(w - 238, 74, 210, 27),
            if self.settings.winter_animation {
                "Снег и сияние: вкл"
            } else {
                "Снег и сияние: выкл"
            },
            true,
        );
        self.winter.lights(dc, w);
        match self.page {
            0 => {
                box_round(dc, rect(x, 124, width, 98), CARD, LINE, 14);
                DrawIconEx(
                    dc,
                    x + 20,
                    147,
                    self.hero_icon,
                    48,
                    48,
                    0,
                    null_mut(),
                    DI_NORMAL,
                );
                let state = if self.running.iter().any(|b| *b) {
                    "Движки работают"
                } else if self.ready {
                    "Готово к подключению"
                } else {
                    "Подготовка компонентов"
                };
                text(
                    dc,
                    state,
                    rect(x + 84, 143, width - 254, 29),
                    20,
                    TEXT,
                    DT_SINGLELINE,
                );
                text(
                    dc,
                    "Доступность проверяется отдельно",
                    rect(x + 84, 178, width - 240, 22),
                    12,
                    MUTED,
                    DT_SINGLELINE,
                );
                self.button(
                    1,
                    rect(x + width - 145, 153, 122, 39),
                    if self.running.iter().any(|b| *b) {
                        "Отключить всё"
                    } else {
                        "Подключить"
                    },
                    self.ready
                        && !self.busy.iter().any(|b| *b)
                        && (self.settings.telegram
                            || self.settings.zapret
                            || self.running.iter().any(|b| *b)),
                );
                let cw = (width - 16) / 2;
                for i in 0..2 {
                    let cx = x + i as i32 * (cw + 16);
                    box_round(dc, rect(cx, 246, cw, 224), CARD, LINE, 14);
                    text(
                        dc,
                        if i == 0 {
                            "Telegram"
                        } else {
                            "Discord + YouTube"
                        },
                        rect(cx + 20, 268, cw - 64, 29),
                        21,
                        TEXT,
                        DT_SINGLELINE,
                    );
                    let selected = if i == 0 {
                        self.settings.telegram
                    } else {
                        self.settings.zapret
                    };
                    self.button(
                        15 + i as u32,
                        rect(cx + cw - 44, 270, 25, 25),
                        if selected { "✓" } else { "" },
                        !self.busy[i],
                    );
                    text(
                        dc,
                        if i == 0 {
                            "MTProto → WebSocket · Rust"
                        } else {
                            "Служба Windows · Flowseal"
                        },
                        rect(cx + 20, 307, cw - 30, 22),
                        13,
                        MUTED,
                        DT_SINGLELINE,
                    );
                    text(
                        dc,
                        if self.busy[i] {
                            "Подождите…"
                        } else if self.running[i] {
                            "● Движок запущен"
                        } else {
                            "○ Выключен"
                        },
                        rect(cx + 20, 349, cw - 30, 24),
                        13,
                        if self.running[i] { ACCENT } else { MUTED },
                        DT_SINGLELINE,
                    );
                    self.button(
                        10 + i as u32,
                        rect(cx + 20, 388, cw - 40, 37),
                        if self.running[i] {
                            "Остановить"
                        } else {
                            "Включить"
                        },
                        self.ready && !self.busy[i],
                    );
                    if i == 0 {
                        self.button(
                            12,
                            rect(cx + 20, 433, cw - 40, 25),
                            "Подключить в Telegram ↗",
                            self.running[0] && !self.busy[0],
                        );
                    } else {
                        text(
                            dc,
                            &format!(
                                "Профиль: {}",
                                relay::strategy_label(&self.settings.strategy)
                            ),
                            rect(cx + 20, 436, cw - 40, 22),
                            12,
                            MUTED,
                            DT_SINGLELINE,
                        );
                    }
                }
                let mut y = 491;
                if !self.admin {
                    box_round(
                        dc,
                        rect(x, y, width, 50),
                        rgb(45, 37, 24),
                        rgb(45, 37, 24),
                        9,
                    );
                    text(
                        dc,
                        "Для zapret и AI нужны права администратора",
                        rect(x + 14, y + 16, width - 270, 24),
                        13,
                        rgb(237, 202, 139),
                        DT_SINGLELINE,
                    );
                    self.button(
                        13,
                        rect(x + width - 242, y + 8, 226, 34),
                        "Перезапустить с правами ↗",
                        true,
                    );
                    y += 70;
                }
                self.button(
                    14,
                    rect(x, y, 148, 38),
                    if self.checking {
                        "Проверяем…"
                    } else {
                        "Проверить сеть"
                    },
                    !self.checking && !self.alt_busy,
                );
                text(
                    dc,
                    "Без установки · без аккаунта · без подписки",
                    rect(x + 166, y + 10, width - 168, 26),
                    12,
                    MUTED,
                    DT_SINGLELINE,
                );
                text(
                    dc,
                    "Работоспособность зависит от сети и выбранной стратегии.",
                    rect(x, y + 59, width, 24),
                    12,
                    MUTED,
                    DT_SINGLELINE,
                );
            }
            1 => {
                self.button(
                    14,
                    rect(x, 124, 184, 39),
                    if self.checking {
                        "Проверяем…"
                    } else {
                        "Запустить проверку"
                    },
                    !self.checking,
                );
                self.button(
                    if self.alt_busy { 91 } else { 90 },
                    rect(x + 224, 124, 185, 36),
                    if self.alt_busy {
                        "Отменить подбор"
                    } else {
                        "Подобрать ALT"
                    },
                    self.alt_busy || (self.admin && self.ready && !self.busy.iter().any(|b| *b)),
                );
                self.button(
                    92,
                    rect(x + 423, 124, 210, 36),
                    if self.diagnosing {
                        "Диагностика…"
                    } else {
                        "Диагностика конфликтов"
                    },
                    !self.diagnosing,
                );
                if self.show_diagnostics {
                    if self.findings.is_empty() {
                        text(
                            dc,
                            "Проверяем службы, адаптеры и порт…",
                            rect(x, 185, width, 30),
                            15,
                            MUTED,
                            DT_SINGLELINE,
                        );
                    }
                    for (i, finding) in self.findings.iter().take(4).enumerate() {
                        let y = 183 + i as i32 * 110;
                        box_round(dc, rect(x, y, width, 96), CARD, LINE, 12);
                        text(
                            dc,
                            &finding.title,
                            rect(x + 18, y + 12, width - 36, 25),
                            17,
                            if finding.warning {
                                rgb(239, 190, 113)
                            } else {
                                ACCENT
                            },
                            DT_SINGLELINE,
                        );
                        text(
                            dc,
                            &finding.detail,
                            rect(x + 18, y + 43, width - 36, 46),
                            13,
                            MUTED,
                            DT_WORDBREAK,
                        );
                    }
                } else {
                    if self.checks.is_empty() {
                        text(
                            dc,
                            "Здесь появятся результаты проверки.",
                            rect(x, 192, width, 30),
                            15,
                            MUTED,
                            DT_SINGLELINE,
                        );
                    }
                    for (i, c) in self.checks.iter().enumerate() {
                        let y = 186 + i as i32 * 94;
                        box_round(dc, rect(x, y, width, 78), CARD, LINE, 12);
                        text(
                            dc,
                            &format!("{}  {}", if c.ok { "●" } else { "○" }, c.name),
                            rect(x + 20, y + 13, width - 160, 28),
                            18,
                            if c.ok { ACCENT } else { rgb(242, 159, 139) },
                            DT_SINGLELINE,
                        );
                        text(
                            dc,
                            &format!("{} мс", c.ms),
                            rect(x + width - 120, y + 15, 100, 24),
                            13,
                            MUTED,
                            DT_RIGHT | DT_SINGLELINE,
                        );
                        text(
                            dc,
                            &c.detail,
                            rect(x + 20, y + 46, width - 40, 23),
                            12,
                            MUTED,
                            DT_SINGLELINE | DT_END_ELLIPSIS,
                        );
                    }
                    text(
                        dc,
                        "Telegram Web проверяется напрямую, отдельно от MTProto. HTTPS-проверка не подтверждает видео и голос Discord — проверьте их в приложениях.",
                        rect(x, 500, width, 85),
                        14,
                        MUTED,
                        DT_WORDBREAK,
                    );
                }
            }
            2 => {
                self.button(30, rect(x, 124, 140, 36), "Копировать", true);
                self.button(31, rect(x + 154, 124, 112, 36), "Очистить", true);
                box_round(dc, rect(x, 181, width, h - 260), CARD, LINE, 12);
                let lines = ((h - 294) / 23).max(1) as usize;
                let end = self.logs.len().saturating_sub(self.log_offset);
                let start = end.saturating_sub(lines);
                for (i, line) in self.logs.iter().skip(start).take(end - start).enumerate() {
                    text(
                        dc,
                        line,
                        rect(x + 16, 197 + i as i32 * 23, width - 32, 22),
                        12,
                        MUTED,
                        DT_SINGLELINE | DT_END_ELLIPSIS,
                    );
                }
                text(
                    dc,
                    "Прокрутка — колёсиком мыши",
                    rect(x, h - 66, width, 20),
                    11,
                    MUTED,
                    DT_SINGLELINE,
                );
            }
            3 => {
                let idle = !self.running.iter().any(|b| *b)
                    && !self.busy.iter().any(|b| *b)
                    && !self.startup_busy;
                box_round(dc, rect(x, 124, width, 288), CARD, LINE, 14);
                text(
                    dc,
                    "Стратегия Discord и YouTube",
                    rect(x + 20, 145, width - 40, 28),
                    18,
                    TEXT,
                    DT_SINGLELINE,
                );
                text(
                    dc,
                    "Выберите ALT. Для запуска используется служба Windows.",
                    rect(x + 20, 183, width - 40, 25),
                    13,
                    MUTED,
                    DT_SINGLELINE,
                );
                self.button(
                    20,
                    rect(x + 20, 223, 340, 38),
                    &format!("{}  ▾", relay::strategy_label(&self.settings.strategy)),
                    idle && self.ready,
                );
                text(
                    dc,
                    "Порт Telegram · 127.0.0.1",
                    rect(x + 20, 286, width - 40, 25),
                    16,
                    TEXT,
                    DT_SINGLELINE,
                );
                box_round(dc, rect(x + 20, 321, 138, 38), rgb(12, 27, 27), LINE, 8);
                place_control(self.hwnd, self.port_edit, rect(x + 30, 330, 118, 21));
                if (IsWindowEnabled(self.port_edit) != 0) != idle {
                    EnableWindow(self.port_edit, if idle { 1 } else { 0 });
                }
                self.button(
                    21,
                    rect(x + 174, 321, 145, 38),
                    "Сохранить",
                    idle && self.ready,
                );
                if !idle {
                    text(
                        dc,
                        "Отключите движки, чтобы изменить настройки.",
                        rect(x + 20, 376, width - 40, 22),
                        12,
                        ACCENT,
                        DT_SINGLELINE,
                    );
                }
                box_round(dc, rect(x, 435, width, 160), CARD, LINE, 14);
                text(
                    dc,
                    "Запуск и восстановление",
                    rect(x + 20, 453, width - 40, 25),
                    18,
                    TEXT,
                    DT_SINGLELINE,
                );
                self.button(
                    60,
                    rect(x + 20, 490, 360, 35),
                    if self.startup_busy {
                        "Обновление…"
                    } else if self.settings.auto_start {
                        "✓  Запускать при входе в Windows"
                    } else {
                        "○  Запускать при входе в Windows"
                    },
                    self.ready && self.admin,
                );
                self.button(
                    61,
                    rect(x + 20, 539, 360, 35),
                    if self.settings.auto_connect {
                        "✓  Восстанавливать включённые сервисы"
                    } else {
                        "○  Восстанавливать включённые сервисы"
                    },
                    self.ready,
                );
                if !self.admin {
                    self.button(
                        13,
                        rect(x + width - 245, 490, 225, 35),
                        "Перезапуск с правами ↗",
                        true,
                    );
                }
                text(
                    dc,
                    "Сначала включите нужные сервисы. Их состояние запоминается автоматически.",
                    rect(x, 604, width, 23),
                    12,
                    MUTED,
                    DT_SINGLELINE,
                );
                box_round(dc, rect(x, 638, width, 78), CARD, LINE, 14);
                text(
                    dc,
                    "Подключение Telegram",
                    rect(x + 20, 645, width - 40, 20),
                    18,
                    TEXT,
                    DT_SINGLELINE,
                );
                self.button(
                    12,
                    rect(x + 20, 672, 175, 32),
                    "Открыть Telegram ↗",
                    self.running[0],
                );
                self.button(22, rect(x + 211, 672, 185, 32), "Копировать ссылку", true);
            }
            4 => {
                box_round(dc, rect(x, 124, width, 230), CARD, LINE, 14);
                text(
                    dc,
                    "ChatGPT · Claude · Gemini",
                    rect(x + 20, 145, width - 40, 30),
                    22,
                    TEXT,
                    DT_SINGLELINE,
                );
                text(
                    dc,
                    "Провайдер: dns.malw.link · внешний SNI-прокси",
                    rect(x + 20, 187, width - 40, 25),
                    14,
                    ACCENT,
                    DT_SINGLELINE,
                );
                text(
                    dc,
                    "Выбранные AI-домены направляются через публичный сервер. Маршрут действует для приложений этого компьютера. Нужны права администратора; при отключении Relay удаляет только собственный блок hosts.",
                    rect(x + 20, 224, width - 40, 66),
                    14,
                    MUTED,
                    DT_WORDBREAK,
                );
                self.button(
                    50,
                    rect(x + 20, 302, 236, 35),
                    if self.ai_busy {
                        "Подождите…"
                    } else if self.ai_running {
                        "Отключить AI-маршрут"
                    } else {
                        "Включить AI-маршрут"
                    },
                    self.ready && !self.ai_busy,
                );
                text(
                    dc,
                    if self.ai_running {
                        "● Маршрут настроен"
                    } else {
                        "○ Выключен"
                    },
                    rect(x + 278, 310, width - 300, 24),
                    13,
                    if self.ai_running { ACCENT } else { MUTED },
                    DT_SINGLELINE,
                );
                box_round(dc, rect(x, 374, width, 122), CARD, LINE, 12);
                text(
                    dc,
                    "После включения перезапустите браузер",
                    rect(x + 20, 393, width - 40, 28),
                    18,
                    TEXT,
                    DT_SINGLELINE,
                );
                text(
                    dc,
                    "Проверка доступности без аккаунта не подтверждает вход и генерацию. Сервис может применять ограничения к аккаунту или адресу сервера. Сертификаты и настройки TLS не изменяются.",
                    rect(x + 20, 434, width - 40, 51),
                    14,
                    MUTED,
                    DT_WORDBREAK,
                );
                self.button(45, rect(x, 517, 150, 37), "Открыть ChatGPT ↗", true);
                self.button(46, rect(x + 166, 517, 150, 37), "Открыть Claude ↗", true);
                self.button(47, rect(x + 332, 517, 150, 37), "Открыть Gemini ↗", true);
                self.button(42, rect(x, 577, 150, 34), "О провайдере ↗", true);
                self.button(
                    43,
                    rect(x + 166, 577, 200, 34),
                    "Свой сервер: EuroDNS ↗",
                    true,
                );
                text(
                    dc,
                    "Бэкап hosts сохраняется в Relay_data. Список маршрутов встроен в exe; автоматических загрузок нет.",
                    rect(x, 635, width, 45),
                    12,
                    MUTED,
                    DT_WORDBREAK,
                );
            }
            5 => {
                box_round(dc, rect(x, 124, width, 230), CARD, LINE, 14);
                text(
                    dc,
                    &format!("Установлена версия {}", env!("CARGO_PKG_VERSION")),
                    rect(x + 20, 147, width - 40, 30),
                    22,
                    TEXT,
                    DT_SINGLELINE,
                );
                text(
                    dc,
                    &self.update_status,
                    rect(x + 20, 196, width - 40, 70),
                    14,
                    MUTED,
                    DT_WORDBREAK,
                );
                self.button(
                    81,
                    rect(x + 20, 291, 185, 38),
                    "Проверить сейчас",
                    !self.update_busy,
                );
                self.button(
                    83,
                    rect(x + 221, 291, 250, 38),
                    "Перезапустить и обновить",
                    self.update_ready.is_some()
                        && !self.startup_busy
                        && !self.ai_busy
                        && !self.busy.iter().any(|b| *b),
                );
                box_round(dc, rect(x, 375, width, 160), CARD, LINE, 14);
                self.button(
                    80,
                    rect(x + 20, 399, 390, 38),
                    if self.settings.auto_update {
                        "✓  Скачивать новые релизы автоматически"
                    } else {
                        "○  Скачивать новые релизы автоматически"
                    },
                    true,
                );
                text(
                    dc,
                    "Автоматическая проверка только при запуске. Скачивание идёт в фоне; перед установкой проверяются размер, Windows x64 и SHA-256 GitHub. Настройки сохраняются.",
                    rect(x + 20, 457, width - 40, 63),
                    14,
                    MUTED,
                    DT_WORDBREAK,
                );
                text(
                    dc,
                    "Для выпуска обновления: тег vX.Y.Z и файл Relay.exe в опубликованном релизе GitHub. Предварительные релизы тоже поддерживаются.",
                    rect(x, 560, width, 70),
                    13,
                    MUTED,
                    DT_WORDBREAK,
                );
            }
            6 => {
                box_round(dc, rect(x, 124, width, 350), CARD, LINE, 14);
                text(
                    dc,
                    &format!("Плотность снега · {}", self.settings.snow_count),
                    rect(x + 24, 149, width - 48, 30),
                    19,
                    TEXT,
                    DT_SINGLELINE,
                );
                text(
                    dc,
                    "Нет снега",
                    rect(x + 24, 226, width - 48, 24),
                    12,
                    MUTED,
                    DT_SINGLELINE,
                );
                text(
                    dc,
                    "Больше снега",
                    rect(x + 24, 226, width - 48, 24),
                    12,
                    MUTED,
                    DT_SINGLELINE | DT_RIGHT,
                );
                text(
                    dc,
                    &format!("Плавность · {} кадров/с", self.settings.animation_fps),
                    rect(x + 24, 285, width - 48, 30),
                    19,
                    TEXT,
                    DT_SINGLELINE,
                );
                text(
                    dc,
                    "Экономнее",
                    rect(x + 24, 362, width - 48, 24),
                    12,
                    MUTED,
                    DT_SINGLELINE,
                );
                text(
                    dc,
                    "Плавнее",
                    rect(x + 24, 362, width - 48, 24),
                    12,
                    MUTED,
                    DT_SINGLELINE | DT_RIGHT,
                );
                place_control(
                    self.hwnd,
                    self.sliders[0],
                    rect(x + 24, 190, width - 48, 32),
                );
                place_control(
                    self.hwnd,
                    self.sliders[1],
                    rect(x + 24, 326, width - 48, 32),
                );
                text(
                    dc,
                    "Настройки сохраняются автоматически. При скрытии окна и потере фокуса анимация останавливается. Размер буфера отрисовки не зависит от плотности снега или FPS; высокая плавность увеличивает нагрузку на процессор.",
                    rect(x + 24, 401, width - 48, 63),
                    13,
                    MUTED,
                    DT_WORDBREAK,
                );
            }
            _ => {
                box_round(dc, rect(x, 124, width, 430), CARD, LINE, 14);
                text(
                    dc,
                    "Rust внутри. Win32 снаружи.",
                    rect(x + 20, 146, width - 40, 30),
                    22,
                    TEXT,
                    DT_SINGLELINE,
                );
                text(
                    dc,
                    "Telegram-движок встроен в тот же exe как Rust-библиотека. Python и отдельный установщик не нужны. Запущенные движки включены в показатель RAM.",
                    rect(x + 20, 194, width - 40, 78),
                    14,
                    MUTED,
                    DT_WORDBREAK,
                );
                self.button(
                    40,
                    rect(x + 20, 287, width - 40, 37),
                    "tg-ws-proxy-rs / valnesfjord ↗",
                    true,
                );
                self.button(
                    41,
                    rect(x + 20, 336, width - 40, 37),
                    "zapret-discord-youtube / Flowseal ↗",
                    true,
                );
                text(
                    dc,
                    "Relay — независимый проект. Авторство движков сохранено в лицензиях. Компоненты zapret автоматически распаковываются рядом с exe; при выходе процессы останавливаются.",
                    rect(x + 20, 397, width - 40, 90),
                    14,
                    MUTED,
                    DT_WORDBREAK,
                );
                text(
                    dc,
                    "Крестик скрывает окно. Полный выход — через меню значка в трее.",
                    rect(x + 20, 504, width - 40, 28),
                    13,
                    ACCENT,
                    DT_SINGLELINE,
                );
                self.button(71, rect(x, 577, 160, 36), "Полностью выйти", true);
            }
        }
        for (id, hwnd) in &self.buttons {
            if !self.visible.contains(id) && IsWindowVisible(*hwnd) != 0 {
                ShowWindow(*hwnd, SW_HIDE);
            }
        }
        fill(dc, rect(204, h - 43, w - 204, 43), PANEL);
        fill(dc, rect(204, h - 44, w - 204, 1), LINE);
        text(
            dc,
            &self.message,
            rect(221, h - 34, w - 237, 30),
            12,
            if self.error {
                rgb(245, 165, 147)
            } else {
                MUTED
            },
            DT_WORDBREAK,
        );
    }
}
unsafe fn fill(dc: HDC, r: RECT, c: u32) {
    let b = CreateSolidBrush(c);
    FillRect(dc, &r, b);
    DeleteObject(b);
}
unsafe fn round_control(hwnd: HWND, width: i32, height: i32) {
    let region = CreateRoundRectRgn(0, 0, width + 1, height + 1, 16, 16);
    if !region.is_null() && SetWindowRgn(hwnd, region, 1) == 0 {
        DeleteObject(region);
    }
}
unsafe fn place_control(parent: HWND, hwnd: HWND, r: RECT) {
    let mut old = RECT::default();
    GetWindowRect(hwnd, &mut old);
    let mut point = POINT {
        x: old.left,
        y: old.top,
    };
    ScreenToClient(parent, &mut point);
    if point.x != r.left
        || point.y != r.top
        || old.right - old.left != r.right - r.left
        || old.bottom - old.top != r.bottom - r.top
    {
        MoveWindow(hwnd, r.left, r.top, r.right - r.left, r.bottom - r.top, 1);
    }
}
unsafe fn box_round(dc: HDC, r: RECT, c: u32, border: u32, radius: i32) {
    let b = CreateSolidBrush(c);
    let p = CreatePen(PS_SOLID, 1, border);
    let ob = SelectObject(dc, b);
    let op = SelectObject(dc, p);
    RoundRect(dc, r.left, r.top, r.right, r.bottom, radius * 2, radius * 2);
    SelectObject(dc, op);
    SelectObject(dc, ob);
    DeleteObject(p);
    DeleteObject(b);
}
unsafe fn text(dc: HDC, s: &str, mut r: RECT, size: i32, color: u32, flags: u32) {
    let f = CreateFontW(
        -size,
        0,
        0,
        0,
        400,
        0,
        0,
        0,
        DEFAULT_CHARSET as u32,
        OUT_DEFAULT_PRECIS as u32,
        CLIP_DEFAULT_PRECIS as u32,
        CLEARTYPE_QUALITY as u32,
        DEFAULT_PITCH as u32,
        wide("Segoe UI").as_ptr(),
    );
    let old = SelectObject(dc, f);
    SetBkMode(dc, TRANSPARENT as i32);
    SetTextColor(dc, color);
    let value = wide(s);
    DrawTextW(
        dc,
        value.as_ptr(),
        (value.len() - 1) as i32,
        &mut r,
        flags | DT_NOPREFIX,
    );
    SelectObject(dc, old);
    DeleteObject(f);
}
unsafe fn open_url(s: &str) {
    windows_sys::Win32::UI::Shell::ShellExecuteW(
        null_mut(),
        null(),
        wide(s).as_ptr(),
        null(),
        null(),
        SW_SHOWNORMAL,
    );
}
unsafe fn clipboard(hwnd: HWND, s: &str) -> Result<(), String> {
    if OpenClipboard(hwnd) == 0 {
        return Err("Буфер обмена занят".into());
    }
    let bytes = wide(s);
    let mem = GlobalAlloc(GMEM_MOVEABLE, bytes.len() * 2);
    if mem.is_null() {
        CloseClipboard();
        return Err("Недостаточно памяти".into());
    }
    let p = GlobalLock(mem);
    if p.is_null() {
        GlobalFree(mem);
        CloseClipboard();
        return Err("Буфер обмена недоступен".into());
    }
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), p as *mut u16, bytes.len());
    GlobalUnlock(mem);
    EmptyClipboard();
    let result = SetClipboardData(13, mem);
    CloseClipboard();
    if result.is_null() {
        GlobalFree(mem);
        Err("Не удалось скопировать".into())
    } else {
        Ok(())
    }
}
unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        let create = &*(lp as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
    }
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App;
    if ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    let app = &mut *ptr;
    if app.taskbar_created != 0 && msg == app.taskbar_created {
        app.tray_added = app.add_tray();
        if !app.tray_added {
            app.show_window();
        }
        return 0;
    }
    match msg {
        WM_CREATE => {
            app.hwnd = hwnd;
            app.taskbar_created = RegisterWindowMessageW(wide("TaskbarCreated").as_ptr());
            // Explorer runs without elevation while Relay may run as administrator.
            // Allow only our harmless UI callbacks and Explorer's restart notice.
            ChangeWindowMessageFilterEx(hwnd, TRAY_MESSAGE, MSGFLT_ALLOW, null_mut());
            ChangeWindowMessageFilterEx(hwnd, SHOW_MESSAGE, MSGFLT_ALLOW, null_mut());
            if app.taskbar_created != 0 {
                ChangeWindowMessageFilterEx(hwnd, app.taskbar_created, MSGFLT_ALLOW, null_mut());
            }
            app.tray_added = app.add_tray();
            let raw = hwnd as isize;
            app.backend = Some(Backend::spawn(app.data.clone(), move || {
                PostMessageW(raw as HWND, WM_APP + 1, 0, 0);
            }));
            app.port_edit = CreateWindowExW(
                0,
                wide("EDIT").as_ptr(),
                wide(&app.settings.port.to_string()).as_ptr(),
                WS_CHILD | WS_TABSTOP | ES_NUMBER as u32 | ES_AUTOHSCROLL as u32,
                0,
                0,
                128,
                30,
                hwnd,
                999usize as HMENU,
                GetModuleHandleW(null()),
                null(),
            );
            app.port_font = CreateFontW(
                -15,
                0,
                0,
                0,
                400,
                0,
                0,
                0,
                DEFAULT_CHARSET as u32,
                OUT_DEFAULT_PRECIS as u32,
                CLIP_DEFAULT_PRECIS as u32,
                CLEARTYPE_QUALITY as u32,
                DEFAULT_PITCH as u32,
                wide("Segoe UI").as_ptr(),
            );
            SendMessageW(app.port_edit, WM_SETFONT, app.port_font as usize, 1);
            InitCommonControlsEx(&INITCOMMONCONTROLSEX {
                dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
                dwICC: ICC_BAR_CLASSES,
            });
            for (i, (min, max, value)) in [
                (0, 240, app.settings.snow_count),
                (15, 60, app.settings.animation_fps),
            ]
            .iter()
            .enumerate()
            {
                let slider = CreateWindowExW(
                    0,
                    wide("msctls_trackbar32").as_ptr(),
                    null(),
                    WS_CHILD | WS_TABSTOP | TBS_NOTICKS,
                    0,
                    0,
                    500,
                    32,
                    hwnd,
                    (1100 + i) as HMENU,
                    GetModuleHandleW(null()),
                    null(),
                );
                SendMessageW(slider, TBM_SETRANGEMIN, 0, *min as isize);
                SetWindowTheme(slider, wide("").as_ptr(), wide("").as_ptr());
                SendMessageW(slider, TBM_SETRANGEMAX, 0, *max as isize);
                SendMessageW(slider, TBM_SETPOS, 1, *value as isize);
                SendMessageW(slider, TBM_SETPAGESIZE, 0, if i == 0 { 20 } else { 5 });
                app.sliders[i] = slider;
            }
            SetTimer(hwnd, 1, 500, None);
            0
        }
        WM_TIMER if wp == 2 => {
            if app.animation_timer {
                InvalidateRect(hwnd, null(), 0);
            }
            0
        }
        WM_TIMER => {
            if app.drain() {
                InvalidateRect(hwnd, null(), 0);
            }
            0
        }
        TRAY_MESSAGE => {
            match (lp as u32) & 0xffff {
                NIN_SELECT | WM_LBUTTONDBLCLK => app.show_window(),
                event if event == (NIN_SELECT | 1) => app.show_window(),
                WM_CONTEXTMENU | WM_RBUTTONUP => app.tray_menu(),
                _ => {}
            }
            0
        }
        SHOW_MESSAGE => {
            app.show_window();
            0
        }
        message if message == WM_APP + 4 => {
            app.refresh_animation();
            0
        }
        WM_APP..=0xBFFF => {
            if app.drain() {
                InvalidateRect(hwnd, null(), 0);
            }
            0
        }
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let dc = BeginPaint(hwnd, &mut ps);
            let mut r = RECT::default();
            GetClientRect(hwnd, &mut r);
            if app
                .surface
                .as_ref()
                .is_none_or(|s| s.width != r.right || s.height != r.bottom)
            {
                app.surface = crate::winter::Surface::new(dc, r.right, r.bottom);
            }
            let Some(surface) = app.surface.as_ref() else {
                EndPaint(hwnd, &ps);
                return 0;
            };
            let mem = surface.dc;
            app.draw(mem, r.right, r.bottom);
            BitBlt(dc, 0, 0, r.right, r.bottom, mem, 0, 0, SRCCOPY);
            EndPaint(hwnd, &ps);
            0
        }
        WM_DRAWITEM => {
            let item = &*(lp as *const DRAWITEMSTRUCT);
            let id = item.CtlID;
            let selected = (100..=107).contains(&id) && id - 100 == app.page as u32;
            let primary = id == 1 || id == 21;
            let disabled = item.itemState & ODS_DISABLED != 0;
            let color = if primary {
                ACCENT
            } else if selected {
                rgb(27, 52, 49)
            } else {
                rgb(34, 44, 57)
            };
            let mut r = item.rcItem;
            // Owner-drawn buttons still receive an erased rectangular surface.
            // Paint the parent colour before drawing the rounded silhouette.
            let background = match id {
                100..=107 => PANEL,
                1 | 10..=12 | 15..=16 | 20..=22 | 40..=41 | 50 | 60..=61 | 80..=81 | 83 => CARD,
                13 => {
                    if app.page == 0 {
                        rgb(45, 37, 24)
                    } else {
                        CARD
                    }
                }
                _ => BG,
            };
            fill(item.hDC, r, background);
            let color = if item.itemState & ODS_SELECTED != 0 {
                if primary {
                    rgb(87, 202, 167)
                } else {
                    rgb(46, 63, 77)
                }
            } else {
                color
            };
            box_round(item.hDC, r, color, color, 8);
            let mut buf = [0u16; 256];
            let n = GetWindowTextW(item.hwndItem, buf.as_mut_ptr(), 256);
            text(
                item.hDC,
                &String::from_utf16_lossy(&buf[..n as usize]),
                r,
                14,
                if disabled {
                    rgb(99, 114, 132)
                } else if primary {
                    BG
                } else if selected {
                    ACCENT
                } else {
                    TEXT
                },
                DT_CENTER | DT_VCENTER | DT_SINGLELINE,
            );
            if item.itemState & ODS_FOCUS != 0
                && SendMessageW(hwnd, WM_QUERYUISTATE, 0, 0) as u32 & UISF_HIDEFOCUS == 0
            {
                InflateRect(&mut r, -3, -3);
                let pen = CreatePen(PS_SOLID, 1, ACCENT);
                let old_pen = SelectObject(item.hDC, pen);
                let old_brush = SelectObject(item.hDC, GetStockObject(NULL_BRUSH));
                RoundRect(item.hDC, r.left, r.top, r.right, r.bottom, 12, 12);
                SelectObject(item.hDC, old_brush);
                SelectObject(item.hDC, old_pen);
                DeleteObject(pen);
            }
            1
        }
        WM_HSCROLL if app.sliders.contains(&(lp as HWND)) => {
            let slider = lp as HWND;
            let value = SendMessageW(slider, TBM_GETPOS, 0, 0) as u16;
            if slider == app.sliders[0] {
                app.settings.snow_count = value.min(240);
            } else {
                app.settings.animation_fps = value.clamp(15, 60);
                app.refresh_animation();
            }
            // Save at release/end or after keyboard changes, never for every drag sample.
            if (wp & 0xffff) as u32 != TB_THUMBTRACK {
                app.save();
            }
            InvalidateRect(hwnd, null(), 0);
            0
        }
        WM_NOTIFY if app.sliders.contains(&(*(lp as *const NMHDR)).hwndFrom) => {
            let header = &*(lp as *const NMHDR);
            if header.code == NM_CUSTOMDRAW {
                let item = &*(lp as *const NMCUSTOMDRAW);
                if item.dwDrawStage == CDDS_PREPAINT {
                    fill(item.hdc, item.rc, CARD);
                    return CDRF_NOTIFYITEMDRAW as isize;
                }
                if item.dwDrawStage == CDDS_ITEMPREPAINT {
                    match item.dwItemSpec as u32 {
                        TBCD_CHANNEL => {
                            let mut r = item.rc;
                            let center = (r.top + r.bottom) / 2;
                            r.top = center - 3;
                            r.bottom = center + 3;
                            box_round(item.hdc, r, LINE, LINE, 3);
                        }
                        TBCD_THUMB => box_round(item.hdc, item.rc, ACCENT, ACCENT, 7),
                        _ => {}
                    }
                    return CDRF_SKIPDEFAULT as isize;
                }
            }
            0
        }
        WM_COMMAND => {
            if (wp & 0xffff) == 999 && (wp >> 16) == EN_CHANGE as usize {
                let mut buf = [0u16; 16];
                let n = GetWindowTextW(app.port_edit, buf.as_mut_ptr(), 16);
                if let Ok(port) = String::from_utf16_lossy(&buf[..n as usize]).parse::<u16>()
                    && port >= 1024
                    && port != app.settings.port
                    && !app.startup_busy
                {
                    let old = app.settings.port;
                    app.settings.port = port;
                    if !app.save() {
                        app.settings.port = old;
                    }
                }
            } else if (wp >> 16) == BN_CLICKED as usize {
                app.action((wp & 0xffff) as u32);
            }
            0
        }
        WM_ERASEBKGND => 1,
        WM_SHOWWINDOW | WM_ACTIVATE | WM_SETTINGCHANGE => {
            PostMessageW(hwnd, WM_APP + 4, 0, 0);
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_SIZE => {
            InvalidateRect(hwnd, null(), 0);
            0
        }
        WM_SYSCOMMAND if (wp as u32 & 0xfff0) == SC_MINIMIZE && app.tray_added => {
            ShowWindow(hwnd, SW_HIDE);
            0
        }
        WM_GETMINMAXINFO => {
            let info = &mut *(lp as *mut MINMAXINFO);
            info.ptMinTrackSize = POINT { x: 980, y: 810 };
            0
        }
        WM_MOUSEWHEEL => {
            if app.page == 2 {
                let delta = ((wp >> 16) as u16) as i16;
                if delta > 0 {
                    app.log_offset = (app.log_offset + 3).min(app.logs.len().saturating_sub(1));
                } else {
                    app.log_offset = app.log_offset.saturating_sub(3);
                }
                InvalidateRect(hwnd, null(), 0);
            }
            0
        }
        WM_CTLCOLORSTATIC if app.sliders.contains(&(lp as HWND)) => {
            let dc = wp as HDC;
            SetBkColor(dc, CARD);
            SetDCBrushColor(dc, CARD);
            GetStockObject(DC_BRUSH) as isize
        }
        WM_CTLCOLOREDIT | WM_CTLCOLORSTATIC if lp as HWND == app.port_edit => {
            let dc = wp as HDC;
            SetTextColor(
                dc,
                if IsWindowEnabled(app.port_edit) != 0 {
                    TEXT
                } else {
                    MUTED
                },
            );
            SetBkColor(dc, rgb(12, 27, 27));
            SetDCBrushColor(dc, rgb(12, 27, 27));
            GetStockObject(DC_BRUSH) as isize
        }
        WM_CLOSE => {
            if app.quitting && app.alt_busy {
                if let Some(backend) = &app.backend {
                    backend
                        .cancel_alt
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                }
                app.quitting = false;
                app.show_window();
                app.notify(
                    "Подбор отменяется. Дождитесь восстановления профиля перед выходом.".into(),
                    true,
                );
                return 0;
            }
            if !app.quitting && app.tray_added {
                ShowWindow(hwnd, SW_HIDE);
                return 0;
            }
            if app.startup_busy {
                app.show_window();
                app.notify("Дождитесь обновления автозапуска.".into(), true);
                InvalidateRect(hwnd, null(), 0);
                return 0;
            }
            if app.ai_busy {
                app.show_window();
                app.notify("Дождитесь завершения изменения AI-маршрута.".into(), true);
                InvalidateRect(hwnd, null(), 0);
                return 0;
            }
            if app.busy[1] {
                app.show_window();
                app.notify("Дождитесь запуска или остановки службы.".into(), true);
                return 0;
            }
            if app.running[1] {
                if app.send(Action::Stop(Kind::Zapret)) {
                    app.close_after_service = true;
                    app.busy[1] = true;
                }
                return 0;
            }
            if app.ai_running {
                if app.send(Action::Stop(Kind::Ai)) {
                    app.close_after_ai = true;
                    app.ai_busy = true;
                }
                return 0;
            }
            if app.install_pending {
                let result = app
                    .update_ready
                    .as_ref()
                    .ok_or("Обновление не готово".to_string())
                    .and_then(|d| relay::updates::install(d, &app.data));
                if let Err(e) = result {
                    app.install_pending = false;
                    app.quitting = false;
                    app.show_window();
                    app.notify(e, true);
                    return 0;
                }
            }
            DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => {
            DeleteObject(app.port_font);
            app.remove_tray();
            app.backend.take();
            KillTimer(hwnd, 1);
            KillTimer(hwnd, 2);
            PostQuitMessage(0);
            0
        }
        WM_QUERYENDSESSION => 1,
        WM_ENDSESSION => {
            if wp != 0 {
                app.quitting = true;
                DestroyWindow(hwnd);
            }
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}
unsafe fn load_icon(pixels: i32) -> HICON {
    let bytes = include_bytes!("../assets/relay.ico");
    let count = u16::from_le_bytes(bytes[4..6].try_into().unwrap()) as usize;
    let entry = (0..count)
        .map(|i| 6 + i * 16)
        .min_by_key(|&i| {
            let width = if bytes[i] == 0 { 256 } else { bytes[i] as i32 };
            // Exact-size images win; otherwise prefer downsampling over enlargement.
            if width >= pixels {
                width - pixels
            } else {
                1024 + pixels - width
            }
        })
        .unwrap();
    let size = u32::from_le_bytes(bytes[entry + 8..entry + 12].try_into().unwrap());
    let offset = u32::from_le_bytes(bytes[entry + 12..entry + 16].try_into().unwrap()) as usize;
    CreateIconFromResourceEx(
        bytes[offset..].as_ptr(),
        size,
        1,
        0x00030000,
        pixels,
        pixels,
        LR_DEFAULTCOLOR,
    )
}

pub fn run() {
    unsafe {
        let _instance = match platform::Instance::acquire() {
            Ok(instance) => instance,
            Err(e) => {
                let existing =
                    FindWindowW(wide("RelayNativeWindow").as_ptr(), wide("Relay").as_ptr());
                if !existing.is_null() {
                    PostMessageW(existing, SHOW_MESSAGE, 0, 0);
                    SetForegroundWindow(existing);
                    return;
                }
                MessageBoxW(
                    null_mut(),
                    wide(&e).as_ptr(),
                    wide("Relay").as_ptr(),
                    MB_ICONINFORMATION,
                );
                return;
            }
        };
        let instance = GetModuleHandleW(null());
        let name = wide("RelayNativeWindow");
        let icon = load_icon(64);
        let small_icon = load_icon(16);
        let sidebar_icon = load_icon(32);
        let hero_icon = load_icon(48);
        let class = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            hIcon: icon,
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            lpszClassName: name.as_ptr(),
            ..std::mem::zeroed()
        };
        RegisterClassW(&class);
        let mut app = Box::new(App::new(sidebar_icon, hero_icon, small_icon));
        let hwnd = CreateWindowExW(
            0,
            name.as_ptr(),
            wide("Relay").as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1100,
            810,
            null_mut(),
            null_mut(),
            instance,
            app.as_mut() as *mut App as _,
        );
        if hwnd.is_null() {
            MessageBoxW(
                null_mut(),
                wide("Не удалось создать окно Relay").as_ptr(),
                wide("Relay").as_ptr(),
                MB_ICONERROR,
            );
            return;
        }
        let dark: i32 = 1;
        windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute(
            hwnd,
            20,
            &dark as *const _ as _,
            std::mem::size_of_val(&dark) as u32,
        );
        SendMessageW(hwnd, WM_SETICON, ICON_SMALL as usize, small_icon as isize);
        SendMessageW(
            hwnd,
            WM_CHANGEUISTATE,
            (UIS_SET | (UISF_HIDEFOCUS << 16)) as usize,
            0,
        );
        if !std::env::args().any(|a| a == "--autorun") || !app.tray_added {
            ShowWindow(hwnd, SW_SHOW);
            UpdateWindow(hwnd);
        }
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            if IsDialogMessageW(hwnd, &msg) == 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        for icon in [icon, small_icon, sidebar_icon, hero_icon] {
            if !icon.is_null() {
                DestroyIcon(icon);
            }
        }
    }
}
