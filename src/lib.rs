pub mod ai;
pub mod bundle;
pub mod diagnostics;
pub mod engine;
pub mod platform;
#[cfg(windows)]
pub mod service;
pub mod startup;
pub mod updates;

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub telegram: bool,
    pub zapret: bool,
    pub strategy: String,
    pub port: u16,
    pub secret: String,
    pub auto_start: bool,
    pub auto_connect: bool,
    pub resume_telegram: bool,
    pub resume_zapret: bool,
    pub resume_ai: bool,
    pub auto_update: bool,
    pub winter_animation: bool,
    pub snow_count: u16,
    pub animation_fps: u16,
}

impl Default for Settings {
    fn default() -> Self {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).expect("Windows secure random generator unavailable");
        Self {
            telegram: true,
            zapret: true,
            strategy: "general (ALT).bat".into(),
            port: 1443,
            secret: bytes.iter().map(|b| format!("{b:02x}")).collect(),
            auto_start: false,
            auto_connect: false,
            resume_telegram: false,
            resume_zapret: false,
            resume_ai: false,
            auto_update: true,
            winter_animation: true,
            snow_count: 180,
            animation_fps: 30,
        }
    }
}

impl Settings {
    pub fn validate(&self) -> Result<(), String> {
        if self.snow_count > 240 || !(15..=60).contains(&self.animation_fps) {
            return Err("Некорректные настройки анимации".into());
        }
        if self.port < 1024 {
            return Err("Выберите порт от 1024 до 65535".into());
        }
        if self.secret.len() != 32 || !self.secret.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("Некорректный секрет MTProto".into());
        }
        if !safe_strategy(&self.strategy) {
            return Err("Некорректное имя стратегии".into());
        }
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let mut value: Self =
            serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
                .map_err(|e| format!("Не удалось прочитать настройки: {e}"))?;
        if !safe_strategy(&value.strategy)
            && value.strategy.starts_with("general")
            && value.strategy.ends_with(".bat")
            && value
                .strategy
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || " ()_- .".contains(c))
        {
            value.strategy = "general (ALT).bat".into();
        }
        value.validate()?;
        Ok(value)
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        self.validate()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let temp = path.with_extension("json.tmp");
        std::fs::write(
            &temp,
            serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        std::fs::rename(temp, path).map_err(|e| e.to_string())
    }

    pub fn link(&self) -> String {
        format!(
            "tg://proxy?server=127.0.0.1&port={}&secret=dd{}",
            self.port, self.secret
        )
    }

    pub fn restore_plan(&self) -> Vec<engine::Kind> {
        if !self.auto_connect {
            return vec![];
        }
        [
            (self.resume_telegram, engine::Kind::Telegram),
            (self.resume_zapret, engine::Kind::Zapret),
            (self.resume_ai, engine::Kind::Ai),
        ]
        .into_iter()
        .filter_map(|(on, kind)| on.then_some(kind))
        .collect()
    }
}

pub fn safe_strategy(name: &str) -> bool {
    strategy_number(name).is_some()
}

pub fn strategy_number(name: &str) -> Option<u32> {
    let n = name.strip_prefix("general (ALT")?.strip_suffix(").bat")?;
    if n.is_empty() {
        Some(1)
    } else if n.bytes().all(|c| c.is_ascii_digit()) && !n.starts_with('0') {
        n.parse::<u32>().ok().filter(|n| *n >= 2 && *n <= 999)
    } else {
        None
    }
}
pub fn strategy_label(name: &str) -> String {
    strategy_number(name)
        .map(|n| format!("ALT {n}"))
        .unwrap_or_else(|| "ALT 1".into())
}

pub fn data_dir() -> PathBuf {
    // All runtime files stay beside the portable executable, not in the registry.
    std::env::current_exe()
        .unwrap_or_default()
        .parent()
        .unwrap_or(Path::new("."))
        .join("Relay_data")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn secret_is_random_and_links_use_secure_mtproto() {
        let a = Settings::default();
        let b = Settings::default();
        assert_ne!(a.secret, b.secret);
        assert!(a.validate().is_ok());
        assert!(a.link().contains("&secret=dd"));
    }
    #[test]
    fn reject_strategy_paths_and_invalid_ports() {
        for name in [
            "../general.bat",
            "general&calc.bat",
            "general/other.bat",
            "C:\\general.bat",
        ] {
            assert!(!safe_strategy(name));
        }
        assert!(safe_strategy("general (ALT11).bat"));
        let mut s = Settings {
            port: 0,
            ..Settings::default()
        };
        assert!(s.validate().is_err());
        s.port = 1443;
        s.secret = "z".repeat(32);
        assert!(s.validate().is_err());
    }
    #[test]
    fn older_settings_are_compatible_and_do_not_start_network_services() {
        let old = r#"{"telegram":true,"zapret":false,"strategy":"general.bat","port":1443,"secret":"00112233445566778899aabbccddeeff"}"#;
        let s: Settings = serde_json::from_str(old).unwrap();
        assert!(!s.auto_start);
        assert!(!s.auto_connect);
        assert!(s.restore_plan().is_empty());
    }
    #[test]
    fn only_numbered_alt_profiles_are_selectable() {
        for name in [
            "service.bat",
            "general.bat",
            "general (FAKE TLS AUTO).bat",
            "general (ALT01).bat",
            "general (ALT1).bat",
        ] {
            assert!(!safe_strategy(name), "{name}");
        }
        assert_eq!(strategy_number("general (ALT).bat"), Some(1));
        assert_eq!(strategy_number("general (ALT13).bat"), Some(13));
        assert_eq!(strategy_label("general (ALT11).bat"), "ALT 11");
    }
    #[test]
    fn animation_preferences_round_trip_and_reject_excessive_load() {
        let mut settings = Settings {
            snow_count: 0,
            animation_fps: 60,
            ..Settings::default()
        };
        assert!(settings.validate().is_ok());
        let loaded: Settings =
            serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
        assert_eq!((loaded.snow_count, loaded.animation_fps), (0, 60));
        settings.snow_count = 241;
        assert!(settings.validate().is_err());
        settings.snow_count = 180;
        settings.animation_fps = 1000;
        assert!(settings.validate().is_err());
    }
    #[test]
    fn restore_plan_preserves_only_explicit_service_states() {
        let mut s = Settings {
            auto_connect: true,
            resume_telegram: true,
            resume_ai: true,
            ..Settings::default()
        };
        assert_eq!(
            s.restore_plan(),
            vec![engine::Kind::Telegram, engine::Kind::Ai]
        );
        s.auto_connect = false;
        assert!(s.restore_plan().is_empty());
        s.auto_connect = true;
        let restored: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(restored.restore_plan(), s.restore_plan());
        assert_eq!(restored.secret, s.secret);
    }
    #[test]
    fn settings_file_can_be_updated_without_losing_the_proxy_secret() {
        let mut entropy = [0u8; 8];
        getrandom::fill(&mut entropy).unwrap();
        let dir =
            std::env::temp_dir().join(format!("relay-settings-{:x}", u64::from_le_bytes(entropy)));
        let path = dir.join("config.json");
        let mut settings = Settings::default();
        settings.save(&path).unwrap();
        let secret = settings.secret.clone();
        settings.port = 9080;
        settings.auto_connect = true;
        settings.resume_telegram = true;
        settings.strategy = "general (ALT11).bat".into();
        settings.save(&path).unwrap();
        let loaded = Settings::load(&path).unwrap();
        assert_eq!(loaded.secret, secret);
        assert_eq!(loaded.port, 9080);
        assert_eq!(loaded.strategy, "general (ALT11).bat");
        assert_eq!(loaded.restore_plan(), vec![engine::Kind::Telegram]);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
}
