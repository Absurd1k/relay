use std::{
    collections::BTreeMap,
    net::Ipv4Addr,
    path::{Path, PathBuf},
};

pub const PROVIDER: &str = "dns.malw.link";
pub const SNAPSHOT: &str = include_str!("../assets/ai-hosts.txt");
const BEGIN: &[u8] = b"\r\n# BEGIN RELAY AI MANAGED v1\r\n";
const END: &[u8] = b"# END RELAY AI MANAGED v1\r\n";
const ROOTS: &[&str] = &[
    "chatgpt.com",
    "openai.com",
    "oaistatic.com",
    "oaiusercontent.com",
    "claude.ai",
    "anthropic.com",
    "claude.com",
];
const GOOGLE: &[&str] = &[
    "gemini.google.com",
    "geminiweb-pa.clients6.google.com",
    "aistudio.google.com",
    "generativelanguage.googleapis.com",
    "aitestkitchen.withgoogle.com",
    "aisandbox-pa.googleapis.com",
    "webchannel-alkalimakersuite-pa.clients6.google.com",
    "alkalimakersuite-pa.clients6.google.com",
    "assistant-s3-pa.googleapis.com",
    "proactivebackend-pa.googleapis.com",
    "robinfrontend-pa.googleapis.com",
    "labs.google",
    "notebooklm.google.com",
    "jules.google.com",
    "stitch.withgoogle.com",
    "flow.google.com",
    "notebook.google.com",
];

pub fn allowed(domain: &str) -> bool {
    ROOTS
        .iter()
        .any(|r| domain == *r || domain.strip_suffix(r).is_some_and(|p| p.ends_with('.')))
        || GOOGLE.contains(&domain)
}
fn public(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    !ip.is_private()
        && !ip.is_loopback()
        && !ip.is_link_local()
        && !ip.is_unspecified()
        && !ip.is_multicast()
        && !ip.is_broadcast()
        && o[0] != 0
        && o[0] < 224
        && !(o[0] == 100 && (64..=127).contains(&o[1]))
        && !(o[0] == 192 && o[1] == 0 && matches!(o[2], 0 | 2))
        && !(o[0] == 198 && matches!(o[1], 18 | 19 | 51))
        && !(o[0] == 203 && o[1] == 0 && o[2] == 113)
}
pub fn mappings(text: &str) -> Result<BTreeMap<String, Ipv4Addr>, String> {
    let mut result = BTreeMap::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("");
        let mut words = line.split_whitespace();
        let Some(ip) = words.next() else { continue };
        for name in words {
            let name = name.to_ascii_lowercase();
            if !allowed(&name) {
                continue;
            }
            if name.len() > 253
                || !name
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'.' || c == b'-')
            {
                return Err("Некорректное имя AI-домена".into());
            }
            let ip: Ipv4Addr = ip.parse().map_err(|_| "Некорректный адрес AI-прокси")?;
            if !public(ip) {
                return Err("AI-прокси должен использовать публичный IPv4".into());
            }
            if let Some(old) = result.insert(name, ip)
                && old != ip
            {
                return Err("Конфликт адресов в AI-списке".into());
            }
        }
    }
    if !["chatgpt.com", "claude.ai", "gemini.google.com"]
        .iter()
        .all(|d| result.contains_key(*d))
    {
        return Err("В списке отсутствуют основные AI-домены".into());
    }
    Ok(result)
}
fn find(bytes: &[u8], needle: &[u8]) -> Option<usize> {
    bytes.windows(needle.len()).position(|w| w == needle)
}
pub fn strip(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let start = find(bytes, BEGIN);
    let end = find(bytes, END);
    match (start, end) {
        (None, None) => Ok(bytes.to_vec()),
        (Some(a), Some(b)) if a < b => {
            if find(&bytes[a + BEGIN.len()..], BEGIN).is_some()
                || find(&bytes[b + END.len()..], END).is_some()
            {
                return Err("В hosts несколько блоков Relay. Нужна ручная проверка.".into());
            }
            let mut out = bytes[..a].to_vec();
            out.extend_from_slice(&bytes[b + END.len()..]);
            Ok(out)
        }
        _ => Err("Повреждены границы AI-блока в hosts. Файл не изменён.".into()),
    }
}
pub fn compose(original: &[u8], map: &BTreeMap<String, Ipv4Addr>) -> Result<Vec<u8>, String> {
    if original.contains(&0) {
        return Err("Не поддерживается UTF-16 hosts. Файл не изменён.".into());
    }
    let mut base = strip(original)?;
    for line in String::from_utf8_lossy(&base).lines() {
        let mut fields = line.split('#').next().unwrap_or("").split_whitespace();
        fields.next();
        for name in fields {
            if map.contains_key(&name.to_ascii_lowercase()) {
                return Err(format!(
                    "В hosts уже настроен {name}. Существующая запись сохранена; AI не включён."
                ));
            }
        }
    }
    base.extend_from_slice(BEGIN);
    base.extend_from_slice(b"# Provider: dns.malw.link; only selected AI services\r\n");
    for (name, ip) in map {
        base.extend_from_slice(format!("{ip} {name}\r\n").as_bytes());
    }
    base.extend_from_slice(END);
    Ok(base)
}
pub fn system_hosts() -> PathBuf {
    PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into()))
        .join("System32/drivers/etc/hosts")
}

fn replace(path: &Path, expected: &[u8], replacement: &[u8]) -> Result<(), String> {
    use std::io::Write;
    if std::fs::read(path).map_err(|e| e.to_string())? != expected {
        return Err("hosts изменился другим приложением. Повторите действие.".into());
    }
    let temp = path.with_file_name(format!("hosts.relay-{}.tmp", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|e| format!("Не удалось подготовить hosts: {e}"))?;
    let write = file.write_all(replacement).and_then(|_| file.sync_all());
    drop(file);
    if let Err(e) = write {
        let _ = std::fs::remove_file(&temp);
        return Err(e.to_string());
    }
    if std::fs::read(path).map_err(|e| e.to_string())? != expected {
        let _ = std::fs::remove_file(&temp);
        return Err("hosts изменился другим приложением. Файл не заменён.".into());
    }
    #[cfg(windows)]
    let result = {
        use std::os::windows::ffi::OsStrExt;
        let a: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let b: Vec<u16> = temp.as_os_str().encode_wide().chain(Some(0)).collect();
        if unsafe {
            windows_sys::Win32::Storage::FileSystem::ReplaceFileW(
                a.as_ptr(),
                b.as_ptr(),
                std::ptr::null(),
                0,
                std::ptr::null(),
                std::ptr::null(),
            )
        } == 0
        {
            Err(std::io::Error::last_os_error().to_string())
        } else {
            Ok(())
        }
    };
    #[cfg(not(windows))]
    let result = std::fs::rename(&temp, path).map_err(|e| e.to_string());
    let _ = std::fs::remove_file(&temp);
    result.map_err(|e| format!("Не удалось заменить hosts. Проверьте права администратора: {e}"))
}
fn flush() {
    #[cfg(windows)]
    {
        let exe =
            PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into()))
                .join("System32/ipconfig.exe");
        let mut cmd = std::process::Command::new(exe);
        cmd.arg("/flushdns")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let _ = crate::platform::hidden(&mut cmd).status();
    }
}
pub fn recover(path: &Path) -> Result<bool, String> {
    let current = std::fs::read(path).map_err(|e| e.to_string())?;
    let cleaned = strip(&current)?;
    if cleaned == current {
        return Ok(false);
    }
    if !crate::platform::is_admin() {
        return Err("После прошлого запуска остался AI-блок. Перезапустите Relay с правами администратора для восстановления hosts.".into());
    }
    replace(path, &current, &cleaned)?;
    flush();
    Ok(true)
}

pub struct Guard {
    path: PathBuf,
    active: bool,
}
impl Guard {
    pub fn enable(path: PathBuf, data: &Path) -> Result<Self, String> {
        if !crate::platform::is_admin() {
            return Err("Для AI-маршрута нужны права администратора.".into());
        }
        let current = std::fs::read(&path).map_err(|e| e.to_string())?;
        let map = mappings(SNAPSHOT)?;
        let new = compose(&current, &map)?;
        let mut bytes = [0u8; 8];
        getrandom::fill(&mut bytes).map_err(|e| e.to_string())?;
        let suffix: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        std::fs::write(data.join(format!("hosts-backup-{suffix}.txt")), &current)
            .map_err(|e| e.to_string())?;
        replace(&path, &current, &new)?;
        flush();
        Ok(Self { path, active: true })
    }
    pub fn disable(&mut self) -> Result<(), String> {
        if !self.active {
            return Ok(());
        }
        let current = std::fs::read(&self.path).map_err(|e| e.to_string())?;
        let clean = strip(&current)?;
        if clean != current {
            replace(&self.path, &current, &clean)?;
            flush();
        }
        self.active = false;
        Ok(())
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.disable();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn snapshot_covers_services_and_ignores_unrelated_hosts() {
        let m = mappings(SNAPSHOT).unwrap();
        assert!(m.len() > 20);
        assert!(!m.contains_key("instagram.com"));
        assert!(!allowed("evilopenai.com"));
        assert!(!allowed("openai.com.evil.net"));
    }
    #[test]
    fn restoring_preserves_original_bytes_and_later_user_edits() {
        let original = b"\xef\xbb\xbf127.0.0.1 localhost\n# my config\n";
        let m = mappings(SNAPSHOT).unwrap();
        let mut full = compose(original, &m).unwrap();
        assert_eq!(strip(&full).unwrap(), original);
        full.extend_from_slice(b"1.2.3.4 custom.example\n");
        let mut expected = original.to_vec();
        expected.extend_from_slice(b"1.2.3.4 custom.example\n");
        assert_eq!(strip(&full).unwrap(), expected);
    }
    #[test]
    fn conflicting_entries_and_broken_markers_fail_closed() {
        let m = mappings(SNAPSHOT).unwrap();
        assert!(compose(b"8.8.8.8 auth.openai.com\n", &m).is_err());
        assert!(strip(BEGIN).is_err());
        assert!(compose(b"\xff\xfe\0\0", &m).is_err());
    }
    #[test]
    fn rejects_private_addresses_from_provider() {
        let bad = SNAPSHOT.replace("217.60.179.6", "127.0.0.1");
        assert!(mappings(&bad).is_err());
    }
    #[test]
    fn atomic_edit_and_restore_preserve_other_changes_in_a_test_file() {
        let mut bytes = [0u8; 8];
        getrandom::fill(&mut bytes).unwrap();
        let dir =
            std::env::temp_dir().join(format!("relay-ai-test-{:x}", u64::from_le_bytes(bytes)));
        std::fs::create_dir(&dir).unwrap();
        let p = dir.join("hosts");
        let original = b"127.0.0.1 localhost\r\n";
        std::fs::write(&p, original).unwrap();
        let full = compose(original, &mappings(SNAPSHOT).unwrap()).unwrap();
        replace(&p, original, &full).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), full);
        assert!(replace(&p, b"stale data", original).is_err());
        let mut edited = full;
        edited.extend_from_slice(b"1.2.3.4 user.example\r\n");
        std::fs::write(&p, &edited).unwrap();
        let clean = strip(&edited).unwrap();
        replace(&p, &edited, &clean).unwrap();
        assert_eq!(
            std::fs::read(&p).unwrap(),
            [original.as_slice(), b"1.2.3.4 user.example\r\n"].concat()
        );
        std::fs::remove_file(p).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
}
