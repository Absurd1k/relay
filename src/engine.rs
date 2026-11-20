use crate::{Settings, bundle, platform, safe_strategy};
use std::{
    collections::VecDeque,
    io::{BufRead, BufReader},
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc::{self, Receiver, SyncSender},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Telegram,
    Zapret,
    Ai,
}
impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Telegram => "Telegram",
            Self::Zapret => "Discord / YouTube",
            Self::Ai => "Нейросети",
        }
    }
}

pub enum Action {
    Start(Kind, Settings),
    Stop(Kind),
    Check,
    Shutdown,
    Startup(Settings),
    AutoUpdates(bool),
    CheckUpdate,
    AutoAlt(Settings),
    Diagnose(u16, bool),
    SwitchAlt(Settings, Settings),
}
pub enum Event {
    Ready(Result<(PathBuf, Vec<String>), String>),
    State(Kind, bool, String),
    Log(String),
    Checks(Vec<Check>),
    Memory(f64),
    Startup(Result<Settings, String>),
    Update(crate::updates::Status),
    Diagnostics(Vec<crate::diagnostics::Finding>),
    Profile(Result<Settings, String>),
    AltProgress(String),
    AltDone {
        result: Result<Option<String>, String>,
        running: bool,
    },
}
#[derive(Clone)]
pub struct Check {
    pub name: &'static str,
    pub ok: bool,
    pub ms: u128,
    pub detail: String,
}

pub struct Backend {
    pub cancel_alt: Arc<AtomicBool>,
    pub tx: SyncSender<Action>,
    pub rx: Receiver<Event>,
    handle: Option<thread::JoinHandle<()>>,
}
impl Backend {
    pub fn spawn(data: PathBuf, repaint: impl Fn() + Send + Sync + 'static) -> Self {
        let (tx, commands) = mpsc::sync_channel(16);
        let (events, rx) = mpsc::sync_channel(256);
        let repaint = std::sync::Arc::new(repaint);
        let cancel_alt = Arc::new(AtomicBool::new(false));
        let cancelled = cancel_alt.clone();
        let handle = thread::spawn(move || {
            let mut manager = Manager::new(data, events.clone());
            let ready = manager.prepare();
            let _ = events.send(Event::Ready(ready));
            repaint();
            let mut last_memory = Instant::now();
            let update_busy = Arc::new(AtomicBool::new(false));
            loop {
                match commands.recv_timeout(Duration::from_millis(500)) {
                    Ok(Action::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Ok(Action::Start(kind, settings)) => {
                        if let Err(e) = manager.start(kind, settings) {
                            let running = kind == Kind::Ai && manager.ai.is_some();
                            let _ = events.send(Event::State(kind, running, e));
                        }
                        repaint();
                    }
                    Ok(Action::Stop(kind)) => {
                        manager.stop(kind);
                        repaint();
                    }
                    Ok(Action::Startup(settings)) => {
                        let result = crate::startup::configure(settings, &manager.data);
                        let _ = events.send(Event::Startup(result));
                        repaint();
                    }
                    Ok(Action::AutoAlt(settings)) => {
                        let result = manager.auto_alt(settings, &cancelled, &*repaint);
                        let running = manager.service.as_ref().is_some_and(|s| s.running());
                        let _ = events.send(Event::AltDone { result, running });
                        repaint();
                    }
                    Ok(Action::SwitchAlt(previous, next)) => {
                        let result = (|| {
                            if let Some(service) = manager.service.as_mut() {
                                service.stop()?;
                            }
                            manager.service = None;
                            if let Err(e) = manager.start(Kind::Zapret, next.clone()) {
                                return match manager.start(Kind::Zapret, previous) {
                                    Ok(()) => Err(format!(
                                        "Новый ALT не запущен, прежний восстановлен: {e}"
                                    )),
                                    Err(restore) => Err(format!(
                                        "Новый ALT не запущен: {e}. Восстановление: {restore}"
                                    )),
                                };
                            }
                            Ok(next)
                        })();
                        let _ = events.send(Event::Profile(result));
                        repaint();
                    }
                    Ok(Action::Diagnose(port, tg)) => {
                        let sender = events.clone();
                        let wake = repaint.clone();
                        thread::spawn(move || {
                            let _ =
                                sender.send(Event::Diagnostics(crate::diagnostics::run(port, tg)));
                            wake();
                        });
                    }
                    Ok(Action::AutoUpdates(enabled)) => {
                        if enabled {
                            spawn_update(
                                manager.data.clone(),
                                true,
                                events.clone(),
                                repaint.clone(),
                                update_busy.clone(),
                            );
                        }
                    }
                    Ok(Action::CheckUpdate) => {
                        spawn_update(
                            manager.data.clone(),
                            true,
                            events.clone(),
                            repaint.clone(),
                            update_busy.clone(),
                        );
                    }
                    Ok(Action::Check) => {
                        let sender = events.clone();
                        let wake = repaint.clone();
                        thread::spawn(move || {
                            let checks = check_services();
                            let _ = sender.send(Event::Checks(checks));
                            wake();
                        });
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
                if manager.poll() {
                    repaint();
                }
                if last_memory.elapsed() >= Duration::from_secs(5) {
                    let total = platform::memory_mb(std::process::id()).unwrap_or(0.0)
                        + manager
                            .children
                            .iter()
                            .filter_map(|(_, c)| platform::memory_mb(c.id()))
                            .sum::<f64>();
                    #[cfg(windows)]
                    let total = total
                        + manager
                            .service
                            .as_ref()
                            .and_then(|s| s.pid())
                            .and_then(platform::memory_mb)
                            .unwrap_or(0.0);
                    let _ = events.try_send(Event::Memory(total));
                    last_memory = Instant::now();
                    repaint();
                }
            }
            manager.stop(Kind::Telegram);
            manager.stop(Kind::Zapret);
            manager.stop(Kind::Ai);
        });
        Self {
            cancel_alt,
            tx,
            rx,
            handle: Some(handle),
        }
    }
    pub fn send(&self, action: Action) -> Result<(), String> {
        self.tx
            .try_send(action)
            .map_err(|_| "Операция уже выполняется. Повторите через секунду.".into())
    }
}
impl Drop for Backend {
    fn drop(&mut self) {
        self.cancel_alt.store(true, Ordering::Relaxed);
        let _ = self.tx.send(Action::Shutdown);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

struct Manager {
    data: PathBuf,
    root: Option<PathBuf>,
    job: Result<platform::Job, String>,
    children: Vec<(Kind, Child)>,
    events: SyncSender<Event>,
    ai: Option<crate::ai::Guard>,
    #[cfg(windows)]
    service: Option<crate::service::Service>,
}
impl Manager {
    #[cfg(windows)]
    fn auto_alt(
        &mut self,
        settings: Settings,
        cancelled: &AtomicBool,
        wake: &dyn Fn(),
    ) -> Result<Option<String>, String> {
        if !platform::is_admin() {
            return Err("Для подбора ALT перезапустите Relay с правами администратора.".into());
        }
        if crate::service::external_running()? {
            return Err(
                "Работает отдельная служба zapret. Сначала остановите её самостоятельно.".into(),
            );
        }
        let was_running = self.service.as_ref().is_some_and(|s| s.running());
        let profiles: Vec<String> = std::fs::read_dir(
            self.root
                .as_ref()
                .ok_or("Компоненты ещё не готовы")?
                .join("zapret"),
        )
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|s| safe_strategy(s))
        .collect();
        let mut profiles = profiles;
        profiles.sort_by_key(|s| crate::strategy_number(s));
        let attempt = pick_profile(&profiles, cancelled, |index, profile| {
            if let Some(service) = self.service.as_mut() {
                service.stop()?;
            }
            self.service = None;
            let _ = self.events.send(Event::AltProgress(format!(
                "Проверка {} · {}/{}",
                crate::strategy_label(profile),
                index + 1,
                profiles.len()
            )));
            wake();
            let mut candidate = settings.clone();
            candidate.strategy = profile.clone();
            if let Err(e) = self.start(Kind::Zapret, candidate) {
                let _ = self.events.try_send(Event::Log(e));
                return Ok(false);
            }
            for _ in 0..5 {
                if cancelled.load(Ordering::Relaxed) {
                    return Ok(false);
                }
                thread::sleep(Duration::from_millis(100));
            }
            let checks = check_targets(
                &[
                    ("Discord API", "https://discord.com/api/v10/gateway"),
                    ("YouTube", "https://www.youtube.com/generate_204"),
                ],
                Duration::from_secs(6),
            );
            let works = checks.len() == 2 && checks.iter().all(|c| c.ok);
            for check in &checks {
                let _ = self.events.try_send(Event::Log(format!(
                    "{} · {}: {} · {} мс",
                    crate::strategy_label(profile),
                    check.name,
                    check.detail,
                    check.ms
                )));
            }
            let _ = self.events.send(Event::Checks(checks));
            wake();
            Ok(works && self.service.as_ref().is_some_and(|s| s.running()))
        });
        if !matches!(&attempt, Ok(Some(_))) {
            if let Some(service) = self.service.as_mut() {
                service.stop()?;
            }
            self.service = None;
            if was_running {
                self.start(Kind::Zapret, settings)
                    .map_err(|e| format!("Не удалось вернуть прежнее подключение: {e}"))?;
            }
        }
        attempt
    }
    fn new(data: PathBuf, events: SyncSender<Event>) -> Self {
        Self {
            data,
            root: None,
            job: platform::Job::new(),
            children: vec![],
            events,
            ai: None,
            #[cfg(windows)]
            service: None,
        }
    }
    fn prepare(&mut self) -> Result<(PathBuf, Vec<String>), String> {
        let root = bundle::prepare(&self.data)?;
        crate::ai::recover(&crate::ai::system_hosts())?;
        let mut strategies: Vec<String> = std::fs::read_dir(root.join("zapret"))
            .map_err(|e| e.to_string())?
            .filter_map(Result::ok)
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|name| safe_strategy(name))
            .collect();
        strategies.sort_by_key(|n| crate::strategy_number(n).unwrap_or(u32::MAX));
        for name in [
            "list-general-user.txt",
            "list-exclude-user.txt",
            "ipset-exclude-user.txt",
        ] {
            let p = root.join("zapret/lists").join(name);
            if !p.exists() {
                std::fs::write(p, "").map_err(|e| e.to_string())?;
            }
        }
        self.root = Some(root.clone());
        #[cfg(windows)]
        if platform::is_admin() {
            match crate::service::Service::attach() {
                Ok(service) => {
                    self.service = service;
                    if self.service.is_some() {
                        let _ = self.events.send(Event::State(
                            Kind::Zapret,
                            true,
                            "Подключена работающая служба RelayZapret".into(),
                        ));
                    }
                }
                Err(e) => {
                    let _ = self.events.try_send(Event::Log(e));
                }
            }
        }
        Ok((root, strategies))
    }
    fn start(&mut self, kind: Kind, settings: Settings) -> Result<(), String> {
        if kind == Kind::Ai {
            if self.ai.is_none() {
                self.ai = Some(crate::ai::Guard::enable(
                    crate::ai::system_hosts(),
                    &self.data,
                )?);
            }
            let _ = self.events.send(Event::State(Kind::Ai, true, "AI-маршрут настроен через dns.malw.link. Перезапустите браузер и проверьте нужный сервис.".into()));
            return Ok(());
        }
        if kind == Kind::Zapret {
            if !platform::is_admin() {
                return Err("Для службы zapret нужны права администратора.".into());
            }
            settings.validate()?;
            #[cfg(windows)]
            {
                if self.service.as_ref().is_some_and(|s| s.running()) {
                    let _ = self.events.send(Event::State(
                        kind,
                        true,
                        "Служба RelayZapret уже работает".into(),
                    ));
                    return Ok(());
                }
                let root = self.root.as_ref().ok_or("Компоненты ещё не готовы")?;
                let manifest = serde_json::from_str(bundle::MANIFEST).map_err(|e| e.to_string())?;
                bundle::verify(root, &manifest)?;
                let dir = crate::service::prepare_files(root)?;
                let script = std::fs::read_to_string(root.join("zapret").join(&settings.strategy))
                    .map_err(|e| e.to_string())?;
                let args = parse_strategy(&script, &dir)?;
                self.service = Some(crate::service::Service::start(&dir, &args)?);
                let _ = self.events.send(Event::State(
                    kind,
                    true,
                    format!(
                        "Служба RelayZapret запущена · {}",
                        crate::strategy_label(&settings.strategy)
                    ),
                ));
                return Ok(());
            }
            #[cfg(not(windows))]
            return Err("Служба доступна только в Windows".into());
        }
        if self.children.iter().any(|(k, _)| *k == kind) {
            return Ok(());
        }
        settings.validate()?;
        let root = self
            .root
            .as_ref()
            .ok_or("Встроенные компоненты ещё не готовы")?;
        self.job
            .as_ref()
            .map_err(|e| format!("Не удалось создать группу фоновых процессов: {e}"))?;
        // Recheck immutable packaged files before each launch; never execute BAT files.
        let manifest = serde_json::from_str(bundle::MANIFEST).map_err(|e| e.to_string())?;
        bundle::verify(root, &manifest)?;
        let mut command = match kind {
            Kind::Telegram => {
                let addr: SocketAddr = format!("127.0.0.1:{}", settings.port)
                    .parse()
                    .map_err(|_| "Некорректный порт")?;
                if TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok() {
                    return Err(format!(
                        "Порт {} уже занят. Измените его в настройках.",
                        settings.port
                    ));
                }
                let mut cmd = Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
                cmd.args([
                    "--tg-worker",
                    "--host",
                    "127.0.0.1",
                    "--port",
                    &settings.port.to_string(),
                    "--secret",
                    &settings.secret,
                    "--pool-size",
                    "2",
                    "--buf-kb",
                    "16",
                    "--max-connections",
                    "64",
                    "--no-outbound-proxy",
                ]);
                cmd.current_dir(&self.data);
                cmd
            }
            Kind::Zapret | Kind::Ai => unreachable!(),
        };
        command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());
        platform::hidden(&mut command);
        let mut child = command
            .spawn()
            .map_err(|e| format!("Не удалось запустить {}: {e}", kind.name()))?;
        if let Err(e) = self.job.as_ref().unwrap().assign(&child) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("Не удалось закрепить фоновый процесс: {e}"));
        }
        for pipe in [
            child
                .stdout
                .take()
                .map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
            child
                .stderr
                .take()
                .map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
        ]
        .into_iter()
        .flatten()
        {
            let events = self.events.clone();
            let secret = settings.secret.clone();
            thread::spawn(move || {
                // Bound both line length and queue; a noisy engine cannot grow UI memory.
                let mut reader = BufReader::new(pipe);
                let mut bytes = Vec::with_capacity(2048);
                loop {
                    bytes.clear();
                    let mut part = std::io::Read::take(&mut reader, 2048);
                    match part.read_until(b'\n', &mut bytes) {
                        Ok(0) | Err(_) => break,
                        _ => {}
                    }
                    let text = String::from_utf8_lossy(&bytes).replace(&secret, "[secret]");
                    let _ =
                        events.try_send(Event::Log(format!("{} · {}", kind.name(), text.trim())));
                }
            });
        }
        // A running process is not proof of end-to-end availability.
        if kind == Kind::Telegram {
            let mut ready = false;
            let addr: SocketAddr = format!("127.0.0.1:{}", settings.port).parse().unwrap();
            for _ in 0..80 {
                if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                    return Err(format!(
                        "Движок Telegram завершился: {status}. Подробности в журнале."
                    ));
                }
                if TcpStream::connect_timeout(&addr, Duration::from_millis(100)).is_ok() {
                    ready = true;
                    break;
                }
                thread::sleep(Duration::from_millis(100));
            }
            if !ready {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Локальный прокси не запустился за 8 секунд".into());
            }
        } else {
            thread::sleep(Duration::from_millis(600));
            if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                return Err(format!(
                    "zapret завершился: {status}. Проверьте журнал и другие экземпляры обхода."
                ));
            }
        }
        self.children.push((kind, child));
        let _ = self.events.send(Event::State(
            kind,
            true,
            if kind == Kind::Telegram {
                "Прокси запущен. Подключите его в Telegram."
            } else {
                "Движок запущен. Проверьте доступность сервисов."
            }
            .into(),
        ));
        Ok(())
    }
    fn stop(&mut self, kind: Kind) {
        #[cfg(windows)]
        if kind == Kind::Zapret {
            if let Some(service) = &mut self.service
                && let Err(e) = service.stop()
            {
                let _ = self.events.try_send(Event::State(kind, true, e));
                return;
            }
            self.service = None;
            let _ = self
                .events
                .try_send(Event::State(kind, false, "Остановлен".into()));
            return;
        }
        if kind == Kind::Ai {
            if let Some(guard) = &mut self.ai
                && let Err(e) = guard.disable()
            {
                let _ = self.events.try_send(Event::State(
                    Kind::Ai,
                    true,
                    format!("Не удалось восстановить hosts: {e}"),
                ));
                return;
            }
            self.ai = None;
            let _ = self
                .events
                .try_send(Event::State(Kind::Ai, false, "Остановлен".into()));
            return;
        }
        if let Some(i) = self.children.iter().position(|(k, _)| *k == kind) {
            let (_, mut child) = self.children.remove(i);
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = self
            .events
            .try_send(Event::State(kind, false, "Остановлен".into()));
    }
    fn poll(&mut self) -> bool {
        let mut changed = false;
        #[cfg(windows)]
        if self.service.as_ref().is_some_and(|s| !s.running()) {
            self.service = None;
            let _ = self.events.try_send(Event::State(
                Kind::Zapret,
                false,
                "Служба RelayZapret остановилась".into(),
            ));
            changed = true;
        }
        let mut i = 0;
        while i < self.children.len() {
            match self.children[i].1.try_wait() {
                Ok(Some(code)) => {
                    let (kind, _) = self.children.remove(i);
                    let _ = self.events.try_send(Event::State(
                        kind,
                        false,
                        format!("Процесс завершился: {code}"),
                    ));
                    changed = true;
                }
                _ => i += 1,
            }
        }
        changed
    }
}
fn spawn_update(
    data: PathBuf,
    force: bool,
    events: SyncSender<Event>,
    repaint: Arc<impl Fn() + Send + Sync + 'static>,
    busy: Arc<AtomicBool>,
) {
    if busy.swap(true, Ordering::AcqRel) {
        return;
    }
    thread::spawn(move || {
        let result = crate::updates::check(&data, force, |status| {
            let _ = events.try_send(Event::Update(status));
            repaint();
        });
        if let Err(e) = result {
            let _ = events.try_send(Event::Update(crate::updates::Status::Error(e)));
            repaint();
        }
        busy.store(false, Ordering::Release);
    });
}
impl Drop for Manager {
    fn drop(&mut self) {
        for (_, child) in &mut self.children {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Parse only the winws argument list. No cmd.exe, CALL, START or scripts execute.
pub fn parse_strategy(script: &str, dir: &Path) -> Result<Vec<String>, String> {
    let marker = "\"%BIN%winws.exe\"";
    let start = script
        .find(marker)
        .ok_or("В стратегии не найден winws.exe")?
        + marker.len();
    let bin = format!("{}\\", dir.join("bin").display());
    let lists = format!("{}\\", dir.join("lists").display());
    let text = script[start..]
        .replace("^\r\n", " ")
        .replace("^\n", " ")
        .replace("%BIN%", &bin)
        .replace("%LISTS%", &lists)
        .replace("%GameFilterTCP%", "12")
        .replace("%GameFilterUDP%", "12");
    if text.contains('%') {
        return Err("Стратегия содержит неподдерживаемые переменные".into());
    }
    let mut args = vec![];
    let mut arg = String::new();
    let mut quoted = false;
    for c in text.chars() {
        if c == '"' {
            quoted = !quoted;
        } else if c.is_whitespace() && !quoted {
            if !arg.is_empty() {
                args.push(std::mem::take(&mut arg));
            }
        } else {
            arg.push(c);
        }
    }
    if quoted {
        return Err("Незакрытая кавычка в стратегии".into());
    }
    if !arg.is_empty() {
        args.push(arg);
    }
    if args.is_empty()
        || args
            .iter()
            .any(|a| !a.starts_with("--") || a.contains(['&', '|', '>', '<', '\r', '\n']))
    {
        return Err("Неподдерживаемый формат стратегии".into());
    }
    Ok(args)
}

fn pick_profile(
    profiles: &[String],
    cancelled: &AtomicBool,
    mut probe: impl FnMut(usize, &String) -> Result<bool, String>,
) -> Result<Option<String>, String> {
    for (index, profile) in profiles.iter().enumerate() {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let works = probe(index, profile)?;
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        if works {
            return Ok(Some(profile.clone()));
        }
    }
    Err("Ни один ALT не прошёл обе HTTPS-проверки. Прежний профиль восстановлен.".into())
}

fn check_services() -> Vec<Check> {
    let targets = [
        ("Telegram Web", "https://web.telegram.org/"),
        ("Discord API", "https://discord.com/api/v10/gateway"),
        ("YouTube", "https://www.youtube.com/generate_204"),
    ];
    check_targets(&targets, Duration::from_secs(10))
}
fn check_targets(targets: &[(&'static str, &'static str)], timeout: Duration) -> Vec<Check> {
    let handles: Vec<_> = targets
        .iter()
        .copied()
        .map(|(name, url)| {
            thread::spawn(move || {
                let agent: ureq::Agent = ureq::Agent::config_builder()
                    .timeout_global(Some(timeout))
                    .proxy(None)
                    .build()
                    .into();
                let start = Instant::now();
                let result = agent.get(url).call();
                match result {
                    Ok(response) => Check {
                        name,
                        ok: true,
                        ms: start.elapsed().as_millis(),
                        detail: format!("HTTPS {}", response.status()),
                    },
                    Err(e) => Check {
                        name,
                        ok: false,
                        ms: start.elapsed().as_millis(),
                        detail: e.to_string(),
                    },
                }
            })
        })
        .collect();
    handles.into_iter().filter_map(|h| h.join().ok()).collect()
}

pub fn push_log(log: &mut VecDeque<String>, text: String) {
    if log.len() >= 300 {
        log.pop_front();
    }
    log.push_back(text.chars().take(2200).collect());
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn alt_search_stops_at_first_success_and_honours_cancellation() {
        let profiles = vec!["ALT 1".into(), "ALT 2".into(), "ALT 3".into()];
        let cancel = AtomicBool::new(false);
        let mut visited = vec![];
        let result = pick_profile(&profiles, &cancel, |i, _| {
            visited.push(i);
            Ok(i == 1)
        })
        .unwrap();
        assert_eq!(result, Some("ALT 2".into()));
        assert_eq!(visited, vec![0, 1]);
        assert_eq!(
            pick_profile(&profiles, &cancel, |_, _| {
                cancel.store(true, Ordering::Relaxed);
                Ok(true)
            })
            .unwrap(),
            None
        );
        assert_eq!(
            pick_profile(&profiles, &cancel, |_, _| panic!(
                "cancelled search must not probe"
            ))
            .unwrap(),
            None
        );
        cancel.store(false, Ordering::Relaxed);
        assert!(pick_profile(&profiles, &cancel, |_, _| Ok(false)).is_err());
    }
    #[test]
    fn parse_preserves_paths_with_spaces_without_shell_execution() {
        let src = "@echo off\nstart \"x\" /min \"%BIN%winws.exe\" --wf-tcp=443,%GameFilterTCP% ^\n--hostlist=\"%LISTS%list-general.txt\" --new";
        let args = parse_strategy(src, Path::new("C:\\My App")).unwrap();
        assert_eq!(args.len(), 3);
        assert_eq!(args[0], "--wf-tcp=443,12");
        assert!(args[1].contains("My App"));
        assert!(parse_strategy("\"%BIN%winws.exe\" --new & calc", Path::new(".")).is_err());
        assert!(parse_strategy("\"%BIN%winws.exe\" --x=%UNKNOWN%", Path::new(".")).is_err());
    }
    #[test]
    fn journal_is_bounded() {
        let mut log = VecDeque::new();
        for i in 0..1000 {
            push_log(&mut log, i.to_string());
        }
        assert_eq!(log.len(), 300);
        assert_eq!(log.front().unwrap(), "700");
    }
    #[test]
    fn every_bundled_strategy_is_supported() {
        use std::io::Read;
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bundle::PAYLOAD)).unwrap();
        let mut count = 0;
        for i in 0..archive.len() {
            let mut file = archive.by_index(i).unwrap();
            if file.name().starts_with("zapret/general") && file.name().ends_with(".bat") {
                let name = file.name().to_owned();
                let mut script = String::new();
                file.read_to_string(&mut script).unwrap();
                assert!(
                    parse_strategy(&script, Path::new("C:\\Relay data\\zapret")).is_ok(),
                    "{name}"
                );
                count += 1;
            }
        }
        assert!(count >= 10);
    }
}
