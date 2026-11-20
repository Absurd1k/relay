#![allow(unsafe_op_in_unsafe_fn)]
use std::{
    path::{Path, PathBuf},
    ptr::{null, null_mut},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::*,
    System::{Com::CoTaskMemFree, Services::*},
    UI::Shell::{FOLDERID_ProgramFiles, SHGetKnownFolderPath},
};
const NAME: &str = "RelayZapret";
const DISPLAY: &str = "Relay - Discord and YouTube";
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
fn error(context: &str) -> String {
    format!("{context}: {}", std::io::Error::last_os_error())
}
struct Handle(SC_HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseServiceHandle(self.0);
        }
    }
}

pub fn quote(arg: &str) -> String {
    let mut out = String::from("\"");
    let mut slashes = 0;
    for c in arg.chars() {
        if c == '\\' {
            slashes += 1;
            continue;
        }
        if c == '"' {
            out.extend(std::iter::repeat_n('\\', slashes * 2 + 1));
            out.push('"');
        } else {
            out.extend(std::iter::repeat_n('\\', slashes));
            out.push(c);
        }
        slashes = 0;
    }
    out.extend(std::iter::repeat_n('\\', slashes * 2));
    out.push('"');
    out
}

fn program_root() -> Result<PathBuf, String> {
    unsafe {
        let mut ptr = null_mut();
        if SHGetKnownFolderPath(&FOLDERID_ProgramFiles, 0, null_mut(), &mut ptr) < 0 {
            return Err("Не найдена папка Program Files".into());
        }
        let mut len = 0;
        while *ptr.add(len) != 0 {
            len += 1;
        }
        let path = PathBuf::from(String::from_utf16_lossy(std::slice::from_raw_parts(
            ptr, len,
        )))
        .join("RelayPortableZapret");
        CoTaskMemFree(ptr as _);
        Ok(path)
    }
}
fn no_reparse(path: &Path) -> Result<(), String> {
    use std::os::windows::fs::MetadataExt;
    if std::fs::symlink_metadata(path)
        .map_err(|e| e.to_string())?
        .file_attributes()
        & 0x400
        != 0
    {
        return Err("Папка службы не может быть ссылкой или junction".into());
    }
    Ok(())
}
pub fn prepare_files(source: &Path) -> Result<PathBuf, String> {
    if !crate::platform::is_admin() {
        return Err("Для службы нужны права администратора".into());
    }
    let manifest: crate::bundle::Manifest =
        serde_json::from_str(crate::bundle::MANIFEST).map_err(|e| e.to_string())?;
    let base = program_root()?;
    std::fs::create_dir_all(&base).map_err(|e| e.to_string())?;
    no_reparse(&base)?;
    let mut cmd = std::process::Command::new(
        PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into()))
            .join("System32/icacls.exe"),
    );
    cmd.arg(&base).args([
        "/inheritance:r",
        "/grant:r",
        "*S-1-5-18:(OI)(CI)F",
        "*S-1-5-32-544:(OI)(CI)F",
        "*S-1-5-32-545:(OI)(CI)RX",
    ]);
    let result = crate::platform::hidden(&mut cmd)
        .output()
        .map_err(|e| e.to_string())?;
    if !result.status.success() {
        return Err("Не удалось защитить папку службы от изменения без прав администратора".into());
    }
    let dest = base.join(&manifest.bundle_sha256[..16]);
    std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
    no_reparse(&dest)?;
    for (name, expected) in &manifest.files {
        if !(name.starts_with("zapret/bin/") || name.starts_with("zapret/lists/")) {
            continue;
        }
        let relative = crate::bundle::safe_entry(name.strip_prefix("zapret/").unwrap())?;
        let data = std::fs::read(source.join(name)).map_err(|e| e.to_string())?;
        if crate::bundle::digest(&data) != *expected {
            return Err(format!("Повреждён файл службы {name}"));
        }
        let target = dest.join(relative);
        std::fs::create_dir_all(target.parent().unwrap()).map_err(|e| e.to_string())?;
        no_reparse(target.parent().unwrap())?;
        if target.exists() {
            no_reparse(&target)?;
            if std::fs::read(&target).ok().as_deref() == Some(data.as_slice()) {
                continue;
            }
        }
        std::fs::write(target, data).map_err(|e| e.to_string())?;
    }
    for name in [
        "list-general-user.txt",
        "list-exclude-user.txt",
        "ipset-exclude-user.txt",
    ] {
        let p = dest.join("lists").join(name);
        if !p.exists() {
            std::fs::write(p, "").map_err(|e| e.to_string())?;
        }
    }
    Ok(dest)
}
unsafe fn manager(access: u32) -> Result<Handle, String> {
    let h = OpenSCManagerW(null(), null(), access);
    if h.is_null() {
        Err(error("SCM"))
    } else {
        Ok(Handle(h))
    }
}
unsafe fn open(m: &Handle, access: u32) -> Result<Option<Handle>, String> {
    let h = OpenServiceW(m.0, wide(NAME).as_ptr(), access);
    if h.is_null() {
        if GetLastError() == ERROR_SERVICE_DOES_NOT_EXIST {
            Ok(None)
        } else {
            Err(error("Открытие службы Relay"))
        }
    } else {
        Ok(Some(Handle(h)))
    }
}
unsafe fn state(h: &Handle) -> Result<SERVICE_STATUS_PROCESS, String> {
    let mut s: SERVICE_STATUS_PROCESS = std::mem::zeroed();
    let mut needed = 0;
    if QueryServiceStatusEx(
        h.0,
        SC_STATUS_PROCESS_INFO,
        &mut s as *mut _ as _,
        std::mem::size_of_val(&s) as u32,
        &mut needed,
    ) == 0
    {
        Err(error("Статус службы"))
    } else {
        Ok(s)
    }
}
unsafe fn owned(h: &Handle) -> Result<(), String> {
    let mut needed = 0;
    QueryServiceConfigW(h.0, null_mut(), 0, &mut needed);
    if needed == 0 {
        return Err(error("Конфигурация службы"));
    }
    let mut bytes = vec![0usize; (needed as usize).div_ceil(std::mem::size_of::<usize>())];
    let ptr = bytes.as_mut_ptr() as *mut QUERY_SERVICE_CONFIGW;
    if QueryServiceConfigW(h.0, ptr, needed, &mut needed) == 0 {
        return Err(error("Конфигурация службы"));
    }
    let p = (*ptr).lpDisplayName;
    let mut n = 0;
    while *p.add(n) != 0 {
        n += 1;
    }
    if String::from_utf16_lossy(std::slice::from_raw_parts(p, n)) != DISPLAY {
        return Err(
            "Служба RelayZapret уже существует и не принадлежит Relay. Она не изменена.".into(),
        );
    }
    Ok(())
}
pub struct Service {
    handle: Handle,
}
impl Service {
    pub fn start(root: &Path, args: &[String]) -> Result<Self, String> {
        unsafe {
            let m = manager(SC_MANAGER_CONNECT | SC_MANAGER_CREATE_SERVICE)?;
            // Do not alter Flowseal's independent service or its driver.
            let external = OpenServiceW(m.0, wide("zapret").as_ptr(), SERVICE_QUERY_STATUS);
            if !external.is_null() {
                let external = Handle(external);
                if state(&external)?.dwCurrentState == SERVICE_RUNNING {
                    return Err("Уже работает отдельная служба zapret. Остановите её, чтобы избежать конфликта.".into());
                }
            }
            let exe = root.join("bin/winws.exe");
            let command = std::iter::once(quote(&exe.to_string_lossy()))
                .chain(args.iter().map(|s| quote(s)))
                .collect::<Vec<_>>()
                .join(" ");
            let access = SERVICE_QUERY_CONFIG
                | SERVICE_CHANGE_CONFIG
                | SERVICE_START
                | SERVICE_STOP
                | SERVICE_QUERY_STATUS;
            let handle = if let Some(h) = open(&m, access)? {
                owned(&h)?;
                if state(&h)?.dwCurrentState != SERVICE_STOPPED {
                    return Err("Служба Relay уже запущена. Сначала остановите её.".into());
                }
                if ChangeServiceConfigW(
                    h.0,
                    SERVICE_NO_CHANGE,
                    SERVICE_DEMAND_START,
                    SERVICE_NO_CHANGE,
                    wide(&command).as_ptr(),
                    null(),
                    null_mut(),
                    null(),
                    null(),
                    null(),
                    null(),
                ) == 0
                {
                    return Err(error("Настройка службы"));
                }
                h
            } else {
                let h = CreateServiceW(
                    m.0,
                    wide(NAME).as_ptr(),
                    wide(DISPLAY).as_ptr(),
                    access,
                    SERVICE_WIN32_OWN_PROCESS,
                    SERVICE_DEMAND_START,
                    SERVICE_ERROR_NORMAL,
                    wide(&command).as_ptr(),
                    null(),
                    null_mut(),
                    null(),
                    null(),
                    null(),
                );
                if h.is_null() {
                    return Err(error("Установка службы"));
                }
                Handle(h)
            };
            let mut service = Self { handle };
            if StartServiceW(service.handle.0, 0, null()) == 0 {
                return Err(error("Запуск службы"));
            }
            let deadline = Instant::now() + Duration::from_secs(12);
            loop {
                let s = state(&service.handle)?;
                if s.dwCurrentState == SERVICE_RUNNING {
                    return Ok(service);
                }
                if s.dwCurrentState == SERVICE_STOPPED {
                    return Err(format!(
                        "Служба завершилась: код Windows {}, код движка {}",
                        s.dwWin32ExitCode, s.dwServiceSpecificExitCode
                    ));
                }
                if Instant::now() >= deadline {
                    let _ = service.stop();
                    return Err("Служба не запустилась за 12 секунд".into());
                }
                std::thread::sleep(Duration::from_millis(150));
            }
        }
    }
    pub fn attach() -> Result<Option<Self>, String> {
        unsafe {
            let m = manager(SC_MANAGER_CONNECT)?;
            let Some(h) = open(
                &m,
                SERVICE_QUERY_CONFIG | SERVICE_QUERY_STATUS | SERVICE_STOP,
            )?
            else {
                return Ok(None);
            };
            owned(&h)?;
            if state(&h)?.dwCurrentState == SERVICE_RUNNING {
                Ok(Some(Self { handle: h }))
            } else {
                Ok(None)
            }
        }
    }
    pub fn running(&self) -> bool {
        unsafe { state(&self.handle).is_ok_and(|s| s.dwCurrentState == SERVICE_RUNNING) }
    }
    pub fn pid(&self) -> Option<u32> {
        unsafe {
            state(&self.handle)
                .ok()
                .map(|s| s.dwProcessId)
                .filter(|p| *p > 0)
        }
    }
    pub fn stop(&mut self) -> Result<(), String> {
        unsafe {
            if state(&self.handle)?.dwCurrentState == SERVICE_STOPPED {
                return Ok(());
            }
            let mut s: SERVICE_STATUS = std::mem::zeroed();
            if ControlService(self.handle.0, SERVICE_CONTROL_STOP, &mut s) == 0
                && GetLastError() != ERROR_SERVICE_NOT_ACTIVE
            {
                return Err(error("Остановка службы"));
            }
            let deadline = Instant::now() + Duration::from_secs(10);
            while state(&self.handle)?.dwCurrentState != SERVICE_STOPPED {
                if Instant::now() >= deadline {
                    return Err("Служба не остановилась за 10 секунд".into());
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Ok(())
        }
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn command_arguments_preserve_quotes_and_trailing_backslashes() {
        assert_eq!(
            quote("C:\\Program Files\\Relay\\"),
            "\"C:\\Program Files\\Relay\\\\\""
        );
        assert_eq!(quote("a\"b"), "\"a\\\"b\"");
    }
}
