//! Настройки приложения (data\settings.json). Никакого реестра.

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::paths;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ZapretSettings {
    /// Версия установленного релиза (тег GitHub).
    pub installed_version: Option<String>,
    /// Выбранная по итогам перебора стратегия (имя .bat).
    pub selected: Option<String>,
    /// Версия zapret, для которой была выбрана стратегия.
    pub selected_for_version: Option<String>,
    /// Game Filter (порты 1024-65535), как пункт 4 в service.bat.
    pub game_filter: bool,
    /// true — проверить все стратегии и выбрать лучшую; false — до первой полностью рабочей.
    pub full_scan: bool,
    /// Запускать zapret при старте приложения.
    pub autostart: bool,
}

impl Default for ZapretSettings {
    fn default() -> Self {
        Self {
            installed_version: None,
            selected: None,
            selected_for_version: None,
            game_filter: false,
            full_scan: false,
            autostart: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TgSettings {
    pub installed_version: Option<String>,
    pub port: u16,
    /// 32 hex-символа. Генерируется один раз.
    pub secret: Option<String>,
    pub autostart: bool,
}

impl Default for TgSettings {
    fn default() -> Self {
        Self { installed_version: None, port: 1443, secret: None, autostart: true }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub zapret: ZapretSettings,
    pub tg: TgSettings,
    /// Проверять обновления обоих проектов при запуске.
    pub check_updates_on_start: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            zapret: ZapretSettings::default(),
            tg: TgSettings::default(),
            check_updates_on_start: true,
        }
    }
}

static SETTINGS: once_cell::sync::Lazy<Mutex<Settings>> = once_cell::sync::Lazy::new(|| {
    let s = std::fs::read_to_string(paths::settings_file())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    Mutex::new(s)
});

pub fn get() -> Settings {
    SETTINGS.lock().clone()
}

/// Изменяет настройки и сразу сохраняет их на диск.
pub fn update<F: FnOnce(&mut Settings)>(f: F) -> Settings {
    let mut g = SETTINGS.lock();
    f(&mut g);
    let snapshot = g.clone();
    drop(g);
    if let Ok(t) = serde_json::to_string_pretty(&snapshot) {
        let tmp = paths::settings_file().with_extension("json.tmp");
        if std::fs::write(&tmp, t).is_ok() {
            let _ = std::fs::rename(&tmp, paths::settings_file());
        }
    }
    snapshot
}
