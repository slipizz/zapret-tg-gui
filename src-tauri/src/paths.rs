//! Все пути приложения. Всё хранится ТОЛЬКО рядом с exe (portable):
//!
//! ```text
//! ZapretTG.exe
//! data\
//!   settings.json        настройки приложения
//!   zapret\              распакованный релиз Flowseal/zapret-discord-youtube
//!   tgproxy\             TgWsProxy_windows.exe + TgWsProxy_data\ (его portable-конфиг)
//!   downloads\           временные загрузки (очищаются)
//!   logs\                логи приложения
//!   webview\             профиль WebView2 (иначе он лёг бы в %LOCALAPPDATA%)
//! ```

use std::path::{Path, PathBuf};

use once_cell::sync::Lazy;

static ROOT: Lazy<PathBuf> = Lazy::new(|| {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
});

pub fn root() -> &'static Path {
    &ROOT
}

pub fn data() -> PathBuf {
    root().join("data")
}
pub fn settings_file() -> PathBuf {
    data().join("settings.json")
}
pub fn zapret_dir() -> PathBuf {
    data().join("zapret")
}
pub fn tgproxy_dir() -> PathBuf {
    data().join("tgproxy")
}
pub fn downloads() -> PathBuf {
    data().join("downloads")
}
pub fn logs() -> PathBuf {
    data().join("logs")
}
pub fn webview() -> PathBuf {
    data().join("webview")
}

pub fn ensure_all() -> std::io::Result<()> {
    for d in [data(), zapret_dir(), tgproxy_dir(), downloads(), logs(), webview()] {
        std::fs::create_dir_all(d)?;
    }
    Ok(())
}
