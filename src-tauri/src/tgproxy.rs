//! TG Proxy (Flowseal/tg-ws-proxy): официальный TgWsProxy_windows.exe из релиза.
//!
//! Портативность: TgWsProxy сам поддерживает portable-режим — если рядом с exe есть
//! папка `TgWsProxy_data`, конфиг и логи хранятся в ней, а не в %APPDATA%.
//! Приложение заранее пишет туда config.json (порт, secret, отключённое
//! самообновление — обновляет приложение) и отметки первого запуска, поэтому
//! TgWsProxy стартует без собственных диалогов, только со значком в трее.
//!
//! Важно: это ЛОКАЛЬНЫЙ MTProto-прокси. Он слушает 127.0.0.1 и предназначен для
//! Telegram Desktop на этом же компьютере, поэтому в ссылке сервер = 127.0.0.1.

use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Stdio};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde::Serialize;
use serde_json::{json, Value};

use crate::{github, logbus, paths, settings, tlog, winproc};

const HOST: &str = "127.0.0.1";
const DATA_DIR: &str = "TgWsProxy_data";

#[derive(Default)]
struct Runtime {
    child: Option<Child>,
    busy: Option<&'static str>,
    latest: Option<String>,
}

static RT: Lazy<Mutex<Runtime>> = Lazy::new(|| Mutex::new(Runtime::default()));
static OP: Lazy<tokio::sync::Mutex<()>> = Lazy::new(|| tokio::sync::Mutex::new(()));

fn asset_name() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "TgWsProxy_windows_arm64.exe"
    } else {
        "TgWsProxy_windows.exe"
    }
}

fn exe_path() -> PathBuf {
    paths::tgproxy_dir().join(asset_name())
}
fn data_dir() -> PathBuf {
    paths::tgproxy_dir().join(DATA_DIR)
}
fn config_path() -> PathBuf {
    data_dir().join("config.json")
}

pub fn is_installed() -> bool {
    exe_path().is_file()
}

#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub installed: bool,
    pub version: Option<String>,
    pub latest: Option<String>,
    pub running: bool,
    pub listening: bool,
    pub host: String,
    pub port: u16,
    pub secret: String,
    pub link: String,
    pub busy: Option<String>,
    pub autostart: bool,
    pub foreign_running: bool,
}

fn valid_secret(s: &str) -> bool {
    s.len() == 32 && s.chars().all(|c| c.is_ascii_hexdigit())
}

fn new_secret() -> String {
    let b: [u8; 16] = rand::random();
    hex::encode(b)
}

/// Secret хранится в settings.json; при первом запуске берётся из уже существующего
/// конфига TgWsProxy (если есть) или генерируется.
fn secret() -> String {
    if let Some(s) = settings::get().tg.secret.filter(|s| valid_secret(s)) {
        return s;
    }
    let from_cfg = read_config()
        .get("secret")
        .and_then(Value::as_str)
        .filter(|s| valid_secret(s))
        .map(str::to_string);
    let s = from_cfg.unwrap_or_else(new_secret);
    settings::update(|st| st.tg.secret = Some(s.clone()));
    s
}

/// Ссылка в формате, который формирует сам tg-ws-proxy: secret с префиксом `dd`
/// (режим padded intermediate, его использует прокси).
pub fn link(port: u16, secret: &str) -> String {
    format!("tg://proxy?server={HOST}&port={port}&secret=dd{secret}")
}

fn read_config() -> Value {
    std::fs::read_to_string(config_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}))
}

fn write_config() -> Result<()> {
    std::fs::create_dir_all(data_dir())?;
    let s = settings::get();
    let mut cfg = read_config();
    let o = cfg.as_object_mut().ok_or_else(|| anyhow!("config.json повреждён"))?;
    o.insert("host".into(), json!(HOST));
    o.insert("port".into(), json!(s.tg.port));
    o.insert("secret".into(), json!(secret()));
    o.insert("check_updates".into(), json!(false));
    o.insert("autostart".into(), json!(false));
    o.entry("language").or_insert(json!("ru"));
    std::fs::write(config_path(), serde_json::to_string_pretty(&cfg)?)?;
    // Отметки первого запуска: иначе TgWsProxy покажет своё окно приветствия.
    for marker in [".first_run_done_mtproto", ".ipv6_warned"] {
        let p = data_dir().join(marker);
        if !p.exists() {
            let _ = std::fs::write(p, b"");
        }
    }
    Ok(())
}

fn our_procs() -> Vec<winproc::ProcInfo> {
    let d = paths::tgproxy_dir();
    all_procs().into_iter().filter(|p| winproc::exe_inside(p, &d)).collect()
}

fn all_procs() -> Vec<winproc::ProcInfo> {
    let mut v = winproc::find_by_name(asset_name());
    // старый exe, переименованный самообновлением TgWsProxy
    v.extend(winproc::find_by_name(&asset_name().replace(".exe", "_oldtgws.exe")));
    v
}

fn port_open(port: u16) -> bool {
    let addr: SocketAddr = format!("{HOST}:{port}").parse().expect("valid addr");
    TcpStream::connect_timeout(&addr, Duration::from_millis(250)).is_ok()
}

pub fn status() -> Status {
    let s = settings::get();
    let running = !our_procs().is_empty();
    let foreign = all_procs().len() > our_procs().len();
    let sec = secret();
    let rt = RT.lock();
    Status {
        installed: is_installed(),
        version: s.tg.installed_version.clone(),
        latest: rt.latest.clone(),
        running,
        listening: running && port_open(s.tg.port),
        host: HOST.into(),
        port: s.tg.port,
        link: link(s.tg.port, &sec),
        secret: sec,
        busy: rt.busy.map(str::to_string),
        autostart: s.tg.autostart,
        foreign_running: foreign,
    }
}

struct Busy;
impl Busy {
    fn new(w: &'static str) -> Self {
        RT.lock().busy = Some(w);
        logbus::status_changed();
        Busy
    }
}
impl Drop for Busy {
    fn drop(&mut self) {
        RT.lock().busy = None;
        logbus::status_changed();
    }
}

pub async fn start() -> Result<()> {
    let _op = OP.lock().await;
    start_locked().await
}

async fn start_locked() -> Result<()> {
    if !is_installed() {
        bail!("TG Proxy ещё не загружен");
    }
    if !our_procs().is_empty() {
        tlog!(Info, "TG Proxy уже запущен.");
        return Ok(());
    }
    if all_procs().len() > 0 {
        tlog!(Warn, "Уже запущен другой экземпляр TgWsProxy (не из папки приложения). Закройте его — два экземпляра не могут работать одновременно.");
    }
    let port = settings::get().tg.port;
    if port_open(port) {
        bail!("порт {port} уже занят другой программой — смените порт");
    }
    write_config()?;
    tlog!(Info, "Запуск TG Proxy на {HOST}:{port}…");
    let child = winproc::hidden(exe_path())
        .arg("--portable")
        .current_dir(paths::tgproxy_dir())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("не удалось запустить TgWsProxy")?;
    let pid = child.id();
    RT.lock().child = Some(child);

    for _ in 0..60 {
        tokio::time::sleep(Duration::from_millis(250)).await;
        if port_open(port) {
            tlog!(Ok, "TG Proxy работает. Ссылка: {}", link(port, &secret()));
            logbus::status_changed();
            return Ok(());
        }
        if !winproc::is_alive(pid) {
            RT.lock().child = None;
            logbus::status_changed();
            bail!("TgWsProxy завершился при запуске. Подробности: {}", data_dir().join("proxy.log").display());
        }
    }
    tlog!(Warn, "Процесс запущен, но порт {port} пока не отвечает.");
    logbus::status_changed();
    Ok(())
}

pub async fn stop() -> Result<()> {
    let _op = OP.lock().await;
    stop_locked();
    Ok(())
}

fn stop_locked() {
    let was = !our_procs().is_empty();
    if let Some(mut c) = RT.lock().child.take() {
        let _ = c.kill();
        let _ = c.wait();
    }
    for p in our_procs() {
        winproc::kill(p.pid);
    }
    if was {
        tlog!(Info, "TG Proxy остановлен.");
    }
    logbus::status_changed();
}

pub fn stop_blocking() {
    stop_locked();
}

pub async fn restart() -> Result<()> {
    let _op = OP.lock().await;
    stop_locked();
    tokio::time::sleep(Duration::from_millis(500)).await;
    start_locked().await
}

pub async fn update(install: bool) -> Result<bool> {
    let _op = OP.lock().await;
    let _b = Busy::new("update");
    update_locked(install).await
}

async fn update_locked(install: bool) -> Result<bool> {
    tlog!(Info, "Проверка обновлений TG Proxy ({})…", github::TGPROXY_REPO);
    let rel = github::latest_release(github::TGPROXY_REPO).await?;
    RT.lock().latest = Some(rel.tag_name.clone());
    logbus::status_changed();

    let local = settings::get().tg.installed_version;
    if is_installed() {
        if let Some(v) = &local {
            if github::same_version(v, &rel.tag_name) {
                tlog!(Ok, "Установлена актуальная версия: {v}");
                return Ok(false);
            }
        }
        tlog!(Info, "Найдена новая версия: {}", rel.tag_name);
    } else {
        tlog!(Info, "TG Proxy не установлен, последняя версия: {}", rel.tag_name);
    }
    if !install {
        return Ok(false);
    }

    let asset = github::find_asset(&rel, |n| n == asset_name())?.clone();
    tlog!(Info, "Загрузка {} ({:.1} МБ)…", asset.name, asset.size as f64 / 1_048_576.0);
    let tmp_dir = paths::downloads().join("tgproxy");
    let file = github::download_asset(&asset, &tmp_dir, |p| logbus::emit("download-progress", p)).await?;
    tlog!(Ok, "{}", github::digest_note(&asset));

    let was_running = !our_procs().is_empty();
    if was_running {
        tlog!(Info, "Остановка для обновления…");
        stop_locked();
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    tlog!(Info, "Обновление…");
    std::fs::create_dir_all(paths::tgproxy_dir())?;
    let dest = exe_path();
    if dest.exists() {
        std::fs::remove_file(&dest).context("не удалось заменить exe (файл занят?)")?;
    }
    std::fs::rename(&file, &dest).or_else(|_| std::fs::copy(&file, &dest).map(|_| ()))?;
    let _ = std::fs::remove_dir_all(&tmp_dir);
    write_config()?;
    settings::update(|s| s.tg.installed_version = Some(rel.tag_name.clone()));
    tlog!(Ok, "TG Proxy {} установлен.", rel.tag_name);

    if was_running {
        start_locked().await?;
    }
    Ok(true)
}

pub async fn on_app_start() {
    let s = settings::get();
    if !is_installed() || s.check_updates_on_start {
        if let Err(e) = update(true).await {
            tlog!(Error, "Обновление TG Proxy не удалось: {e:#}");
        }
    }
    if is_installed() && settings::get().tg.autostart {
        if let Err(e) = start().await {
            tlog!(Error, "Автозапуск: {e:#}");
        }
    }
}

pub async fn update_and_start() -> Result<()> {
    update(true).await?;
    if our_procs().is_empty() {
        start().await?;
    }
    Ok(())
}

pub async fn set_port(port: u16) -> Result<()> {
    if port < 1024 {
        bail!("используйте порт от 1024 до 65535");
    }
    settings::update(|s| s.tg.port = port);
    tlog!(Info, "Порт изменён на {port}");
    if !our_procs().is_empty() {
        restart().await?;
    } else if is_installed() {
        write_config()?;
    }
    Ok(())
}

pub async fn regenerate_secret() -> Result<()> {
    let s = new_secret();
    settings::update(|st| st.tg.secret = Some(s));
    tlog!(Info, "Создан новый secret — старую ссылку в Telegram нужно заменить.");
    if !our_procs().is_empty() {
        restart().await?;
    } else if is_installed() {
        write_config()?;
    }
    Ok(())
}

/// Транслирует новые строки proxy.log самого TgWsProxy в окно логов TG Proxy.
pub fn spawn_log_tail() {
    std::thread::spawn(|| {
        use std::io::{Read, Seek, SeekFrom};
        let mut offset = 0u64;
        loop {
            std::thread::sleep(Duration::from_millis(1000));
            let path = data_dir().join("proxy.log");
            let Ok(mut f) = std::fs::File::open(&path) else {
                offset = 0;
                continue;
            };
            let len = f.metadata().map(|m| m.len()).unwrap_or(0);
            if len < offset {
                offset = 0; // TgWsProxy пересоздаёт лог при каждом запуске
            }
            if len == offset || f.seek(SeekFrom::Start(offset)).is_err() {
                continue;
            }
            let mut buf = Vec::new();
            if f.read_to_end(&mut buf).is_err() {
                continue;
            }
            // берём только полные строки
            let Some(last_nl) = buf.iter().rposition(|&b| b == b'\n') else { continue };
            offset += last_nl as u64 + 1;
            for line in String::from_utf8_lossy(&buf[..last_nl]).lines() {
                let line = line.trim();
                if !line.is_empty() {
                    logbus::log(logbus::Channel::Tg, logbus::Level::Muted, format!("  proxy: {line}"));
                }
            }
        }
    });
}

pub fn set_autostart(v: bool) {
    settings::update(|s| s.tg.autostart = v);
    logbus::status_changed();
}

pub fn open_in_telegram() -> Result<()> {
    let st = status();
    winproc::shell_open(&st.link).map_err(|_| anyhow!("Telegram не установлен или не зарегистрировал ссылки tg:// — скопируйте ссылку вручную"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn link_format() {
        assert_eq!(
            super::link(1443, "00112233445566778899aabbccddeeff"),
            "tg://proxy?server=127.0.0.1&port=1443&secret=dd00112233445566778899aabbccddeeff"
        );
    }
}
