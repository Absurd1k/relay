use std::process::Command;

pub fn hidden(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    cmd
}

pub fn is_admin() -> bool {
    #[cfg(windows)]
    {
        unsafe { windows_sys::Win32::UI::Shell::IsUserAnAdmin() != 0 }
    }
    #[cfg(not(windows))]
    {
        false
    }
}

pub struct Instance {
    #[cfg(windows)]
    handle: windows_sys::Win32::Foundation::HANDLE,
}
impl Instance {
    pub fn acquire() -> Result<Self, String> {
        #[cfg(windows)]
        {
            unsafe {
                let name = if is_admin() {
                    "Local\\RelayNativeAdmin_v1\0"
                } else {
                    "Local\\RelayNativeUser_v1\0"
                };
                let name: Vec<u16> = name.encode_utf16().collect();
                let handle = windows_sys::Win32::System::Threading::CreateMutexW(
                    std::ptr::null(),
                    0,
                    name.as_ptr(),
                );
                if handle.is_null() {
                    return Err(std::io::Error::last_os_error().to_string());
                }
                if windows_sys::Win32::Foundation::GetLastError()
                    == windows_sys::Win32::Foundation::ERROR_ALREADY_EXISTS
                {
                    windows_sys::Win32::Foundation::CloseHandle(handle);
                    return Err("Relay уже запущен. Откройте существующее окно.".into());
                }
                Ok(Self { handle })
            }
        }
        #[cfg(not(windows))]
        {
            Ok(Self {})
        }
    }
}
impl Drop for Instance {
    fn drop(&mut self) {
        #[cfg(windows)]
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

pub fn elevate() -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let wide: Vec<u16> = exe.as_os_str().encode_wide().chain(Some(0)).collect();
        let verb: Vec<u16> = "runas\0".encode_utf16().collect();
        let result = unsafe {
            windows_sys::Win32::UI::Shell::ShellExecuteW(
                std::ptr::null_mut(),
                verb.as_ptr(),
                wide.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                1,
            )
        };
        if result as isize <= 32 {
            return Err("Запуск отменён или не удалось получить права администратора".into());
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        Err("Поддерживается Windows 10/11 x64".into())
    }
}

pub fn open_telegram(link: &str) -> Result<(), String> {
    if !link.starts_with("tg://proxy?server=127.0.0.1&port=") {
        return Err("Некорректная ссылка".into());
    }
    #[cfg(windows)]
    {
        let wide: Vec<u16> = link.encode_utf16().chain(Some(0)).collect();
        let result = unsafe {
            windows_sys::Win32::UI::Shell::ShellExecuteW(
                std::ptr::null_mut(),
                std::ptr::null(),
                wide.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                1,
            )
        };
        if result as isize <= 32 {
            return Err(
                "Не удалось открыть Telegram. Скопируйте ссылку и откройте её вручную.".into(),
            );
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        Err("Поддерживается Windows".into())
    }
}

pub fn memory_mb(pid: u32) -> Option<f64> {
    #[cfg(windows)]
    {
        unsafe {
            use windows_sys::Win32::{
                Foundation::CloseHandle,
                System::{ProcessStatus::*, Threading::*},
            };
            let handle = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, pid);
            if handle.is_null() {
                return None;
            }
            let mut counters: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
            let ok = K32GetProcessMemoryInfo(
                handle,
                &mut counters,
                std::mem::size_of_val(&counters) as u32,
            );
            CloseHandle(handle);
            if ok == 0 {
                None
            } else {
                Some(counters.WorkingSetSize as f64 / 1048576.0)
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = pid;
        None
    }
}

/// Closing the job handle also stops owned workers if the UI crashes or is killed.
pub struct Job {
    #[cfg(windows)]
    handle: windows_sys::Win32::Foundation::HANDLE,
}
impl Job {
    pub fn new() -> Result<Self, String> {
        #[cfg(windows)]
        {
            unsafe {
                use windows_sys::Win32::{Foundation::CloseHandle, System::JobObjects::*};
                let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if handle.is_null() {
                    return Err(std::io::Error::last_os_error().to_string());
                }
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                if SetInformationJobObject(
                    handle,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as _,
                    std::mem::size_of_val(&info) as u32,
                ) == 0
                {
                    CloseHandle(handle);
                    return Err(std::io::Error::last_os_error().to_string());
                }
                Ok(Self { handle })
            }
        }
        #[cfg(not(windows))]
        {
            Ok(Self {})
        }
    }
    pub fn assign(&self, child: &std::process::Child) -> Result<(), String> {
        #[cfg(windows)]
        {
            unsafe {
                use std::os::windows::io::AsRawHandle;
                if windows_sys::Win32::System::JobObjects::AssignProcessToJobObject(
                    self.handle,
                    child.as_raw_handle() as _,
                ) == 0
                {
                    return Err(std::io::Error::last_os_error().to_string());
                }
            }
        }
        Ok(())
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        #[cfg(windows)]
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}
