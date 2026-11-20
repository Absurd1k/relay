use std::net::TcpListener;

#[derive(Clone, Debug)]
pub struct Finding {
    pub title: String,
    pub detail: String,
    pub warning: bool,
}
pub fn tunnel_hint(name: &str, interface_type: u32) -> bool {
    let name = name.to_ascii_lowercase();
    matches!(interface_type, 23 | 131)
        || [
            "wireguard",
            "wintun",
            "openvpn",
            "vpn",
            "tap-windows",
            "tailscale",
            "zerotier",
            "happ",
            "tun ",
        ]
        .iter()
        .any(|s| name.contains(s))
}

pub fn run(port: u16, telegram_running: bool) -> Vec<Finding> {
    let mut out = vec![];
    #[cfg(windows)]
    {
        match crate::service::external_running() {
            Ok(active) => out.push(Finding { title: "Отдельная служба zapret".into(), warning: active,
                detail: if active { "Работает служба Flowseal zapret. Relay не будет запускать второй движок; остановите отдельную службу самостоятельно." } else { "Работающая отдельная служба zapret не найдена." }.into() }),
            Err(e) => out.push(Finding {title:"Служба zapret".into(),detail:e,warning:true}),
        }
        match processes() {
            Ok(names) => {
                let winws = names
                    .iter()
                    .filter(|s| s.eq_ignore_ascii_case("winws.exe"))
                    .count();
                let own = crate::service::own_running().unwrap_or(false);
                let clients: Vec<_> = names
                    .iter()
                    .filter(|s| {
                        [
                            "happ.exe",
                            "sing-box.exe",
                            "xray.exe",
                            "wireguard.exe",
                            "openvpn.exe",
                            "tailscale-ipn.exe",
                            "v2ray.exe",
                        ]
                        .contains(&s.to_ascii_lowercase().as_str())
                    })
                    .take(4)
                    .cloned()
                    .collect();
                out.push(Finding {
                    title: "Движки и VPN-клиенты".into(),
                    warning: winws > usize::from(own) || !clients.is_empty(),
                    detail: format!(
                        "winws.exe: {winws}. {} VPN-клиенты: {} (процесс не доказывает активное соединение).",
                        if winws > usize::from(own) {
                            "Найден сторонний движок: возможен конфликт."
                        } else {
                            "Лишних движков не обнаружено."
                        }, if clients.is_empty(){"не найдены".into()}else{clients.join(", ")}
                    ),
                });
            }
            Err(e) => out.push(Finding {
                title: "Фоновые движки".into(),
                detail: e,
                warning: true,
            }),
        }
        match tunnels() {
            Ok(names) => out.push(Finding {title:"VPN / туннели".into(),warning:!names.is_empty(),detail:if names.is_empty(){"Активные адаптеры с признаками VPN не найдены. Прокси без адаптера эта проверка не определяет.".into()}else{format!("Возможный VPN: {}. Это признак туннеля, а не доказанный конфликт; сравните работу с ним и без него.",names.join(", "))}}),
            Err(e) => out.push(Finding {title:"VPN / туннели".into(),detail:e,warning:true}),
        }
    }
    let occupied = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).is_err();
    out.push(Finding {title:format!("Порт Telegram · {port}"),warning:occupied && !telegram_running,
        detail:if occupied && telegram_running {"Порт занят при включённом Telegram Relay — это ожидаемо. Владелец сокета отдельно не проверялся.".into()}else if occupied {"Порт занят или недоступен. Выберите другой порт в настройках перед запуском Telegram.".into()}else{"Порт свободен на момент проверки.".into()} });
    out
}

#[cfg(windows)]
fn processes() -> Result<Vec<String>, String> {
    use windows_sys::Win32::{Foundation::*, System::Diagnostics::ToolHelp::*};
    unsafe {
        let h = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if h == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of_val(&entry) as u32;
        let mut out = vec![];
        let mut more = Process32FirstW(h, &mut entry);
        while more != 0 && out.len() < 4096 {
            let n = entry
                .szExeFile
                .iter()
                .position(|c| *c == 0)
                .unwrap_or(entry.szExeFile.len());
            out.push(String::from_utf16_lossy(&entry.szExeFile[..n]));
            more = Process32NextW(h, &mut entry);
        }
        CloseHandle(h);
        Ok(out)
    }
}
#[cfg(windows)]
fn tunnels() -> Result<Vec<String>, String> {
    use windows_sys::Win32::{Foundation::ERROR_BUFFER_OVERFLOW, NetworkManagement::IpHelper::*};
    unsafe {
        let mut bytes = 15000u32;
        for _ in 0..3 {
            if bytes > 1024 * 1024 {
                return Err("Слишком большой список адаптеров".into());
            }
            let mut memory = vec![0usize; (bytes as usize).div_ceil(std::mem::size_of::<usize>())];
            let ptr = memory.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH;
            let result = GetAdaptersAddresses(
                0,
                GAA_FLAG_SKIP_UNICAST
                    | GAA_FLAG_SKIP_ANYCAST
                    | GAA_FLAG_SKIP_MULTICAST
                    | GAA_FLAG_SKIP_DNS_SERVER,
                std::ptr::null(),
                ptr,
                &mut bytes,
            );
            if result == ERROR_BUFFER_OVERFLOW {
                continue;
            }
            if result != 0 {
                return Err(format!("Не удалось получить адаптеры: {result}"));
            }
            let mut cursor = ptr;
            let mut out = vec![];
            let mut count = 0;
            while !cursor.is_null() && count < 1024 {
                let a = &*cursor;
                let read = |p: *const u16| {
                    if p.is_null() {
                        return String::new();
                    }
                    let mut len = 0;
                    while len < 512 && *p.add(len) != 0 {
                        len += 1;
                    }
                    String::from_utf16_lossy(std::slice::from_raw_parts(p, len))
                };
                let name = read(a.FriendlyName);
                if a.OperStatus == 1
                    && tunnel_hint(&format!("{} {}", name, read(a.Description)), a.IfType)
                {
                    out.push(name);
                }
                if out.len() >= 8 {
                    break;
                }
                cursor = a.Next;
                count += 1;
            }
            return Ok(out);
        }
        Err("Список адаптеров изменился, повторите проверку".into())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ordinary_ethernet_is_not_labelled_vpn() {
        assert!(!tunnel_hint("Intel Ethernet", 6));
        assert!(tunnel_hint("WireGuard Tunnel", 6));
        assert!(tunnel_hint("Remote Access", 23));
    }
    #[test]
    fn occupied_port_is_reported_without_stopping_its_owner() {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let report = run(port, false);
        assert!(report.last().unwrap().warning);
        assert!(listener.local_addr().is_ok());
        assert!(!run(port, true).last().unwrap().warning);
    }
}
