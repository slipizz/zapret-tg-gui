//! Команды, вызываемые из интерфейса (ui/app.js → invoke).

use serde::Serialize;

use crate::logbus::{self, Channel, LogLine};
use crate::{deps, paths, settings, tgproxy, winproc, zapret};

type R<T> = Result<T, String>;

fn e<E: std::fmt::Display>(err: E) -> String {
    format!("{err:#}")
}

fn ch(name: &str) -> R<Channel> {
    match name {
        "zapret" => Ok(Channel::Zapret),
        "tg" => Ok(Channel::Tg),
        "app" => Ok(Channel::App),
        _ => Err("неизвестный канал".into()),
    }
}

#[derive(Serialize)]
pub struct AppInfo {
    version: &'static str,
    data_dir: String,
    elevated: bool,
    check_updates_on_start: bool,
}

#[tauri::command]
pub fn app_info() -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION"),
        data_dir: paths::data().display().to_string(),
        elevated: deps::is_elevated(),
        check_updates_on_start: settings::get().check_updates_on_start,
    }
}

#[tauri::command]
pub fn set_check_updates(enabled: bool) {
    settings::update(|s| s.check_updates_on_start = enabled);
}

// ── логи ──

#[tauri::command]
pub fn get_logs(channel: String) -> R<Vec<LogLine>> {
    Ok(logbus::history(ch(&channel)?))
}

#[tauri::command]
pub fn clear_logs(channel: String) -> R<()> {
    logbus::clear(ch(&channel)?);
    Ok(())
}

/// Сохраняет текущий лог в data\logs\ и возвращает путь к файлу.
#[tauri::command]
pub fn save_logs(channel: String) -> R<String> {
    let c = ch(&channel)?;
    let text: String = logbus::history(c)
        .iter()
        .map(|l| format!("[{}] {}\r\n", l.time, l.text))
        .collect();
    let name = format!("{channel}-{}.txt", chrono::Local::now().format("%Y%m%d-%H%M%S"));
    let path = paths::logs().join(name);
    std::fs::write(&path, text).map_err(e)?;
    Ok(path.display().to_string())
}

#[tauri::command]
pub fn open_logs_folder() -> R<()> {
    winproc::shell_open(&paths::logs().to_string_lossy()).map_err(e)
}

// ── zapret ──

#[tauri::command]
pub fn zapret_status() -> zapret::Status {
    zapret::status()
}
#[tauri::command]
pub async fn zapret_start(name: Option<String>) -> R<()> {
    zapret::start(name).await.map_err(e)
}
#[tauri::command]
pub async fn zapret_stop() -> R<()> {
    zapret::stop().await.map_err(e)
}
#[tauri::command]
pub async fn zapret_restart() -> R<()> {
    zapret::restart().await.map_err(e)
}
#[tauri::command]
pub async fn zapret_search() -> R<Option<String>> {
    zapret::search().await.map_err(e)
}
#[tauri::command]
pub fn zapret_cancel() {
    zapret::cancel()
}
#[tauri::command]
pub async fn zapret_update() -> R<()> {
    zapret::update_and_retest().await.map_err(e)
}
#[tauri::command]
pub fn zapret_set_options(game_filter: Option<bool>, full_scan: Option<bool>, autostart: Option<bool>) {
    zapret::set_options(game_filter, full_scan, autostart)
}
#[tauri::command]
pub fn zapret_select(name: String) -> R<()> {
    zapret::select(name).map_err(e)
}
#[tauri::command]
pub fn zapret_open_folder() -> R<()> {
    zapret::open_folder().map_err(e)
}

// ── TG Proxy ──

#[tauri::command]
pub fn tg_status() -> tgproxy::Status {
    tgproxy::status()
}
#[tauri::command]
pub async fn tg_start() -> R<()> {
    tgproxy::start().await.map_err(e)
}
#[tauri::command]
pub async fn tg_stop() -> R<()> {
    tgproxy::stop().await.map_err(e)
}
#[tauri::command]
pub async fn tg_restart() -> R<()> {
    tgproxy::restart().await.map_err(e)
}
#[tauri::command]
pub async fn tg_update() -> R<()> {
    tgproxy::update_and_start().await.map_err(e)
}
#[tauri::command]
pub async fn tg_set_port(port: u16) -> R<()> {
    tgproxy::set_port(port).await.map_err(e)
}
#[tauri::command]
pub async fn tg_regen_secret() -> R<()> {
    tgproxy::regenerate_secret().await.map_err(e)
}
#[tauri::command]
pub fn tg_set_autostart(enabled: bool) {
    tgproxy::set_autostart(enabled)
}
#[tauri::command]
pub fn tg_open_telegram() -> R<()> {
    tgproxy::open_in_telegram().map_err(e)
}

// ── компоненты ──

#[tauri::command]
pub fn deps_list() -> Vec<deps::Component> {
    deps::all()
}

#[tauri::command]
pub async fn deps_install(id: deps::Id) -> R<()> {
    let c = deps::all().into_iter().find(|c| c.id == id).ok_or("нет такого компонента")?;
    deps::install(&c, |m| crate::alog!(Info, "{m}")).await.map_err(e)
}

/// Открывает только заранее известные официальные адреса.
#[tauri::command]
pub fn open_url(url: String) -> R<()> {
    let allowed = deps::all().iter().any(|c| c.page_url == url)
        || url.starts_with("https://github.com/Flowseal/zapret-discord-youtube")
        || url.starts_with("https://github.com/Flowseal/tg-ws-proxy")
        || url.starts_with("https://desktop.telegram.org");
    if !allowed {
        return Err("адрес не входит в список разрешённых".into());
    }
    winproc::shell_open(&url).map_err(e)
}

#[tauri::command]
pub fn deps_open_all_missing() -> R<()> {
    for c in deps::all().iter().filter(|c| !c.installed) {
        winproc::shell_open(c.page_url).map_err(e)?;
    }
    Ok(())
}
