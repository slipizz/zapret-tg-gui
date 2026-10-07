//! Zapret (Flowseal/zapret-discord-youtube): установка/обновление из официального
//! релиза, разбор стратегий general*.bat, запуск winws.exe, автоматический перебор.
//!
//! Как запускается стратегия. Все general*.bat релиза устроены одинаково:
//! служебные вызовы service.bat + одна команда `start "..." /min "%BIN%winws.exe" <аргументы> ^`.
//! Приложение читает этот .bat, подставляет переменные (%BIN%, %LISTS%, %GameFilter*%)
//! ровно так же, как это делает cmd, и запускает winws.exe напрямую. Так:
//!   * не появляется консольных окон и вызов не открывает браузер (check_updates в service.bat);
//!   * у приложения есть PID процесса — остановка надёжная;
//!   * сам .bat не меняется: при обновлении zapret новые стратегии подхватываются автоматически.
//! Если .bat не удалось разобрать (формат изменился), используется запасной путь —
//! запуск самого .bat через cmd.exe с NO_UPDATE_CHECK=1.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde::Serialize;

use crate::logbus::{self, Channel, Level};
use crate::{github, paths, settings, tester, winproc, zlog};

const WINWS: &str = "winws.exe";

// ───────────────────────────── состояние ─────────────────────────────

#[derive(Debug, Clone, Serialize, Default)]
pub struct SearchProgress {
    pub current: usize,
    pub total: usize,
    pub name: String,
}

#[derive(Default)]
struct Runtime {
    child: Option<Child>,
    running_strategy: Option<String>,
    busy: Option<&'static str>,
    progress: Option<SearchProgress>,
    latest: Option<String>,
}

static RT: Lazy<Mutex<Runtime>> = Lazy::new(|| Mutex::new(Runtime::default()));
/// Длинные операции (перебор, обновление, запуск) выполняются строго по одной.
static OP: Lazy<tokio::sync::Mutex<()>> = Lazy::new(|| tokio::sync::Mutex::new(()));
static CANCEL: AtomicBool = AtomicBool::new(false);

struct BusyGuard;
impl BusyGuard {
    fn new(what: &'static str) -> Self {
        RT.lock().busy = Some(what);
        logbus::status_changed();
        BusyGuard
    }
}
impl Drop for BusyGuard {
    fn drop(&mut self) {
        let mut rt = RT.lock();
        rt.busy = None;
        rt.progress = None;
        drop(rt);
        logbus::status_changed();
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub installed: bool,
    pub version: Option<String>,
    pub latest: Option<String>,
    pub running: bool,
    pub running_strategy: Option<String>,
    pub selected: Option<String>,
    pub strategies: Vec<String>,
    pub busy: Option<String>,
    pub progress: Option<SearchProgress>,
    pub game_filter: bool,
    pub full_scan: bool,
    pub autostart: bool,
    pub foreign_winws: bool,
}

pub fn status() -> Status {
    let s = settings::get();
    let installed = is_installed();
    let ours = our_winws();
    let foreign = winproc::find_by_name(WINWS).len() > ours.len();
    let rt = RT.lock();
    Status {
        installed,
        version: if installed { local_version().or(s.zapret.installed_version.clone()) } else { None },
        latest: rt.latest.clone(),
        running: !ours.is_empty(),
        running_strategy: if ours.is_empty() { None } else { rt.running_strategy.clone() },
        selected: s.zapret.selected.clone(),
        strategies: if installed { list_strategies().unwrap_or_default() } else { vec![] },
        busy: rt.busy.map(str::to_string),
        progress: rt.progress.clone(),
        game_filter: s.zapret.game_filter,
        full_scan: s.zapret.full_scan,
        autostart: s.zapret.autostart,
        foreign_winws: foreign,
    }
}

pub fn cancel() {
    CANCEL.store(true, Ordering::SeqCst);
    zlog!(Warn, "Получен запрос на отмену — завершаю текущий шаг…");
}

// ───────────────────────────── файлы релиза ─────────────────────────────

fn dir() -> PathBuf {
    paths::zapret_dir()
}
fn bin_dir() -> PathBuf {
    dir().join("bin")
}
fn lists_dir() -> PathBuf {
    dir().join("lists")
}

pub fn is_installed() -> bool {
    bin_dir().join(WINWS).is_file() && dir().join("service.bat").is_file()
}

/// Версия из service.bat: `set "LOCAL_VERSION=1.10.3"`.
fn local_version() -> Option<String> {
    let text = std::fs::read_to_string(dir().join("service.bat")).ok()?;
    text.lines().find_map(|l| {
        let l = l.trim();
        let rest = l.strip_prefix("set \"LOCAL_VERSION=")?;
        Some(rest.trim_end_matches('"').to_string())
    })
}

/// Все стратегии general*.bat в естественном порядке: general, ALT, ALT2 … ALT13, …
pub fn list_strategies() -> Result<Vec<String>> {
    let mut v: Vec<String> = std::fs::read_dir(dir())?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| {
            let l = n.to_lowercase();
            l.starts_with("general") && l.ends_with(".bat")
        })
        .collect();
    v.sort_by(|a, b| natural_key(a).cmp(&natural_key(b)));
    Ok(v)
}

fn natural_key(s: &str) -> Vec<(u8, u64, String)> {
    // "general (ALT2).bat" → "general  alt2"; general.bat оказывается первым как самый короткий
    let s = s.to_lowercase().replace(".bat", "").replace(['(', ')'], " ");
    let s = s.trim();
    let mut out = Vec::new();
    let mut num = String::new();
    let mut txt = String::new();
    for ch in s.chars() {
        if ch.is_ascii_digit() {
            if !txt.is_empty() {
                out.push((0, 0, std::mem::take(&mut txt)));
            }
            num.push(ch);
        } else {
            if !num.is_empty() {
                out.push((1, num.parse().unwrap_or(0), String::new()));
                num.clear();
            }
            txt.push(ch);
        }
    }
    if !txt.is_empty() {
        out.push((0, 0, txt));
    }
    if !num.is_empty() {
        out.push((1, num.parse().unwrap_or(0), String::new()));
    }
    out
}

/// То же, что `service.bat load_user_lists`: пользовательские списки должны существовать.
fn ensure_user_lists() {
    let l = lists_dir();
    let files: [(&str, &str); 3] = [
        ("ipset-exclude-user.txt", "203.0.113.113/32\r\n"),
        ("list-general-user.txt", "# Never leave this file empty\r\ndomain.example.abc\r\n"),
        ("list-exclude-user.txt", "domain.example.abc\r\n"),
    ];
    for (name, content) in files {
        let p = l.join(name);
        if !p.exists() {
            let _ = std::fs::write(p, content);
        }
    }
}

/// То же, что `service.bat status_zapret` → `:tcp_enable`.
fn ensure_tcp_timestamps() {
    if let Ok((_, out)) = winproc::run_hidden("netsh.exe", &["interface", "tcp", "show", "global"]) {
        let enabled = out
            .lines()
            .any(|l| l.to_lowercase().contains("timestamps") && l.to_lowercase().contains("enabled"));
        if !enabled {
            let _ = winproc::run_hidden("netsh.exe", &["interface", "tcp", "set", "global", "timestamps=enabled"]);
            zlog!(Muted, "Включены TCP timestamps (как в service.bat)");
        }
    }
}

// ───────────────────────────── разбор .bat ─────────────────────────────

/// Значения GameFilter* — как в `service.bat :game_switch_status`.
fn game_filter_values(enabled: bool) -> (&'static str, &'static str) {
    if enabled {
        ("1024-65535", "1024-65535")
    } else {
        // "12" — заглушка из service.bat: фильтр фактически выключен
        ("12", "12")
    }
}

/// Возвращает аргументы winws.exe из стратегии.
pub fn parse_strategy(bat: &Path, game_filter: bool) -> Result<Vec<String>> {
    let raw = std::fs::read(bat).with_context(|| format!("не удалось прочитать {}", bat.display()))?;
    let text = String::from_utf8_lossy(&raw);

    // Склеиваем строки-продолжения (`^` в конце).
    let mut logical: Vec<String> = Vec::new();
    let mut cur = String::new();
    for line in text.lines() {
        let t = line.trim_end();
        if let Some(stripped) = t.strip_suffix('^') {
            cur.push_str(stripped);
            cur.push(' ');
        } else {
            cur.push_str(t);
            logical.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        logical.push(cur);
    }

    let line = logical
        .iter()
        .find(|l| {
            let l = l.trim_start().to_lowercase();
            l.starts_with("start ") && l.contains("winws.exe")
        })
        .ok_or_else(|| anyhow!("в файле нет команды запуска winws.exe"))?;

    let root = format!("{}\\", dir().display());
    let (gtcp, gudp) = game_filter_values(game_filter);
    let name = bat.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let vars: [(&str, String); 7] = [
        ("%BIN%", format!("{}\\", bin_dir().display())),
        ("%LISTS%", format!("{}\\", lists_dir().display())),
        ("%~dp0", root.clone()),
        ("%~n0", name),
        ("%GameFilterTCP%", gtcp.into()),
        ("%GameFilterUDP%", gudp.into()),
        ("%GameFilter%", gtcp.into()),
    ];
    let mut expanded = line.clone();
    for (k, v) in &vars {
        expanded = replace_ci(&expanded, k, v);
    }
    if let Some(i) = expanded.find('%') {
        let tail: String = expanded[i..].chars().take(40).collect();
        bail!("неизвестная переменная в стратегии: {tail}");
    }

    let tokens = split_cmd(&expanded);
    let exe_idx = tokens
        .iter()
        .position(|t| t.to_lowercase().ends_with("winws.exe"))
        .ok_or_else(|| anyhow!("не найден путь к winws.exe"))?;
    let args: Vec<String> = tokens[exe_idx + 1..].to_vec();
    if args.is_empty() {
        bail!("у winws.exe нет аргументов");
    }
    Ok(args)
}

fn replace_ci(hay: &str, needle: &str, with: &str) -> String {
    let lower = hay.to_lowercase();
    let nl = needle.to_lowercase();
    let mut out = String::with_capacity(hay.len());
    let mut i = 0;
    while let Some(pos) = lower[i..].find(&nl) {
        out.push_str(&hay[i..i + pos]);
        out.push_str(with);
        i += pos + nl.len();
    }
    out.push_str(&hay[i..]);
    out
}

/// Разбиение командной строки как cmd/MSVCRT: пробелы разделяют, кавычки группируют и удаляются.
fn split_cmd(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_q = false;
    let mut has = false;
    for ch in s.chars() {
        match ch {
            '"' => {
                in_q = !in_q;
                has = true;
            }
            c if c.is_whitespace() && !in_q => {
                if has || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            c => cur.push(c),
        }
    }
    if has || !cur.is_empty() {
        out.push(cur);
    }
    out
}

// ───────────────────────────── процессы ─────────────────────────────

fn our_winws() -> Vec<winproc::ProcInfo> {
    let d = dir();
    winproc::find_by_name(WINWS).into_iter().filter(|p| winproc::exe_inside(p, &d)).collect()
}

pub fn is_running() -> bool {
    !our_winws().is_empty()
}

fn kill_all_winws(include_foreign: bool) {
    if let Some(mut c) = RT.lock().child.take() {
        let _ = c.kill();
        let _ = c.wait();
    }
    let procs = if include_foreign { winproc::find_by_name(WINWS) } else { our_winws() };
    for p in procs {
        winproc::kill(p.pid);
    }
    // дождаться освобождения WinDivert
    for _ in 0..20 {
        if (if include_foreign { winproc::find_by_name(WINWS) } else { our_winws() }).is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Выгрузить драйвер WinDivert (как `service.bat → Remove Services`).
fn unload_windivert() {
    for svc in ["WinDivert", "WinDivert14"] {
        let _ = winproc::run_hidden("sc.exe", &["stop", svc]);
        let _ = winproc::run_hidden("sc.exe", &["delete", svc]);
    }
}

fn zapret_service_installed() -> bool {
    winproc::run_hidden("sc.exe", &["query", "zapret"]).map(|(code, _)| code == 0).unwrap_or(false)
}

fn spawn_strategy(name: &str, quiet: bool) -> Result<()> {
    let s = settings::get();
    let bat = dir().join(name);
    if !bat.is_file() {
        bail!("стратегия {name} не найдена");
    }
    ensure_user_lists();

    let child = match parse_strategy(&bat, s.zapret.game_filter) {
        Ok(args) => {
            if !quiet {
                zlog!(Muted, "winws.exe: {} аргументов из {name}", args.len());
            }
            winproc::hidden(bin_dir().join(WINWS))
                .args(&args)
                .current_dir(bin_dir())
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .context("не удалось запустить winws.exe")?
        }
        Err(e) => {
            zlog!(Warn, "Не удалось разобрать {name} ({e:#}); запуск через cmd.exe");
            winproc::hidden("cmd.exe")
                .arg("/c")
                .arg(&bat)
                .current_dir(dir())
                .env("NO_UPDATE_CHECK", "1")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .context("не удалось запустить cmd.exe")?
        }
    };
    let mut child = child;
    pipe_output(&mut child);
    let mut rt = RT.lock();
    rt.child = Some(child);
    rt.running_strategy = Some(name.to_string());
    Ok(())
}

fn pipe_output(child: &mut Child) {
    fn forward<R: std::io::Read + Send + 'static>(r: R) {
        std::thread::spawn(move || {
            for line in BufReader::new(r).lines().map_while(|l| l.ok()) {
                let line = line.trim().to_string();
                if !line.is_empty() {
                    logbus::log(Channel::Zapret, Level::Muted, format!("  winws: {line}"));
                }
            }
        });
    }
    if let Some(o) = child.stdout.take() {
        forward(o);
    }
    if let Some(e) = child.stderr.take() {
        forward(e);
    }
}

/// Ждём появления winws и проверяем, что он не упал сразу (неверные аргументы, занятый WinDivert).
async fn wait_ready() -> bool {
    for _ in 0..30 {
        if is_running() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    tokio::time::sleep(Duration::from_millis(1800)).await;
    is_running()
}

// ───────────────────────────── публичные действия ─────────────────────────────

pub async fn start(name: Option<String>) -> Result<()> {
    let _op = OP.lock().await;
    start_locked(name).await
}

async fn start_locked(name: Option<String>) -> Result<()> {
    if !is_installed() {
        bail!("zapret ещё не загружен");
    }
    let name = name
        .or_else(|| settings::get().zapret.selected)
        .ok_or_else(|| anyhow!("стратегия не выбрана — запустите «Проверить конфигурации»"))?;

    if zapret_service_installed() {
        zlog!(Warn, "В системе установлена служба «zapret» (service.bat → Install Service). Она конфликтует с приложением — удалите её через service.bat → Remove Services.");
    }
    let foreign = winproc::find_by_name(WINWS).len() > our_winws().len();
    if foreign {
        zlog!(Warn, "Обнаружен другой запущенный winws.exe — он будет остановлен (два экземпляра конфликтуют).");
    }
    kill_all_winws(true);
    ensure_tcp_timestamps();

    zlog!(Info, "Запуск конфигурации {name}…");
    spawn_strategy(&name, false)?;
    if wait_ready().await {
        zlog!(Ok, "Zapret успешно запущен ({name}).");
        logbus::status_changed();
        Ok(())
    } else {
        kill_all_winws(false);
        logbus::status_changed();
        bail!("winws.exe завершился сразу после запуска — см. строки «winws:» выше")
    }
}

pub async fn stop() -> Result<()> {
    let _op = OP.lock().await;
    stop_locked();
    Ok(())
}

fn stop_locked() {
    let was = is_running();
    kill_all_winws(false);
    unload_windivert();
    RT.lock().running_strategy = None;
    if was {
        zlog!(Info, "Zapret остановлен.");
    }
    logbus::status_changed();
}

/// Синхронная остановка для выхода из приложения.
pub fn stop_blocking() {
    if is_running() {
        kill_all_winws(false);
        unload_windivert();
    }
}

pub async fn restart() -> Result<()> {
    let _op = OP.lock().await;
    let name = RT.lock().running_strategy.clone();
    zlog!(Info, "Перезапуск…");
    kill_all_winws(false);
    start_locked(name).await
}

/// Проверка обновления; при `install` — загрузка и замена файлов.
/// Возвращает true, если версия поменялась.
pub async fn update(install: bool) -> Result<bool> {
    let _op = OP.lock().await;
    let _busy = BusyGuard::new("update");
    update_locked(install).await
}

async fn update_locked(install: bool) -> Result<bool> {
    zlog!(Info, "Проверка обновлений zapret ({})…", github::ZAPRET_REPO);
    let rel = github::latest_release(github::ZAPRET_REPO).await?;
    RT.lock().latest = Some(rel.tag_name.clone());
    logbus::status_changed();

    let local = if is_installed() { local_version() } else { None };
    if let Some(v) = &local {
        if github::same_version(v, &rel.tag_name) {
            zlog!(Ok, "Установлена актуальная версия: {v}");
            return Ok(false);
        }
        zlog!(Info, "Найдена новая версия: {} (установлена {v})", rel.tag_name);
    } else {
        zlog!(Info, "Zapret не установлен, последняя версия: {}", rel.tag_name);
    }
    if !install {
        return Ok(false);
    }

    let asset = github::find_asset(&rel, |n| {
        let n = n.to_lowercase();
        n.starts_with("zapret-discord-youtube") && n.ends_with(".zip")
    })?
    .clone();
    zlog!(Info, "Загрузка {} ({:.1} МБ)…", asset.name, asset.size as f64 / 1_048_576.0);
    let file = github::download_asset(&asset, &paths::downloads(), |p| logbus::emit("download-progress", p)).await?;
    zlog!(Ok, "{}", github::digest_note(&asset));

    let staging = paths::data().join("zapret.new");
    let _ = std::fs::remove_dir_all(&staging);
    let f2 = file.clone();
    let st2 = staging.clone();
    tokio::task::spawn_blocking(move || extract_zip(&f2, &st2)).await??;
    let _ = std::fs::remove_file(&file);

    let new_root = find_release_root(&staging).ok_or_else(|| anyhow!("в архиве нет service.bat — неожиданная структура релиза"))?;
    if !new_root.join("bin").join(WINWS).is_file() {
        bail!("в архиве нет bin\\winws.exe");
    }

    // Пользовательские файлы переносим в новую версию.
    let was_running = is_running();
    if was_running {
        zlog!(Info, "Остановка zapret для замены файлов…");
        kill_all_winws(false);
        unload_windivert();
    }
    if is_installed() {
        for rel_path in [
            "lists/ipset-exclude-user.txt",
            "lists/list-general-user.txt",
            "lists/list-exclude-user.txt",
            "utils/game_filter.enabled",
        ] {
            let from = dir().join(rel_path);
            if from.is_file() {
                let to = new_root.join(rel_path);
                if let Some(p) = to.parent() {
                    let _ = std::fs::create_dir_all(p);
                }
                let _ = std::fs::copy(&from, &to);
            }
        }
    }
    // Встроенная проверка обновлений service.bat не нужна — обновляет приложение.
    let _ = std::fs::remove_file(new_root.join("utils").join("check_updates.enabled"));

    zlog!(Info, "Замена файлов…");
    let old = paths::data().join("zapret.old");
    let _ = std::fs::remove_dir_all(&old);
    if dir().exists() {
        std::fs::rename(dir(), &old).context("не удалось переместить старую версию (файлы заняты?)")?;
    }
    if let Err(e) = std::fs::rename(&new_root, dir()) {
        let _ = std::fs::rename(&old, dir());
        return Err(e).context("не удалось установить новую версию");
    }
    let _ = std::fs::remove_dir_all(&old);
    let _ = std::fs::remove_dir_all(&staging);

    let v = local_version().unwrap_or(rel.tag_name.clone());
    settings::update(|s| s.zapret.installed_version = Some(v.clone()));
    let count = list_strategies().map(|v| v.len()).unwrap_or(0);
    zlog!(Ok, "Zapret {v} установлен. Найдено конфигураций: {count}");
    logbus::status_changed();
    Ok(true)
}

fn extract_zip(file: &Path, dest: &Path) -> Result<()> {
    let f = std::fs::File::open(file)?;
    let mut zip = zip::ZipArchive::new(f).context("архив повреждён")?;
    std::fs::create_dir_all(dest)?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let Some(rel) = entry.enclosed_name() else { bail!("небезопасный путь в архиве: {}", entry.name()) };
        let out = dest.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(p) = out.parent() {
            std::fs::create_dir_all(p)?;
        }
        let mut w = std::fs::File::create(&out)?;
        std::io::copy(&mut entry, &mut w).with_context(|| format!("ошибка распаковки {}", entry.name()))?;
    }
    Ok(())
}

fn find_release_root(dir: &Path) -> Option<PathBuf> {
    if dir.join("service.bat").is_file() {
        return Some(dir.to_path_buf());
    }
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        if e.path().is_dir() {
            if let Some(r) = find_release_root(&e.path()) {
                return Some(r);
            }
        }
    }
    None
}

// ───────────────────────────── перебор стратегий ─────────────────────────────

#[derive(Debug, Clone)]
struct Outcome {
    name: String,
    score: tester::Score,
}

pub async fn search() -> Result<Option<String>> {
    let _op = OP.lock().await;
    let _busy = BusyGuard::new("search");
    CANCEL.store(false, Ordering::SeqCst);
    let r = search_locked().await;
    if let Err(e) = &r {
        zlog!(Error, "Проверка прервана: {e:#}");
    }
    r
}

fn set_progress(current: usize, total: usize, name: &str) {
    RT.lock().progress = Some(SearchProgress { current, total, name: name.to_string() });
    logbus::status_changed();
}

async fn search_locked() -> Result<Option<String>> {
    if !is_installed() {
        bail!("zapret ещё не загружен");
    }
    let s = settings::get();
    zlog!(Info, "");
    zlog!(Info, "Запуск проверки Zapret...");
    let strategies = list_strategies()?;
    let total = strategies.len();
    zlog!(Info, "Найдено конфигураций: {total}");
    if total == 0 {
        bail!("в папке zapret нет файлов general*.bat");
    }
    let targets = tester::load_targets(&dir().join("utils").join("targets.txt"));
    zlog!(
        Muted,
        "Цели проверки: {} адресов × TLS 1.2/1.3 (из utils\\targets.txt){}",
        targets.len(),
        if s.zapret.full_scan { ", режим: полный перебор" } else { ", режим: до первой полностью рабочей" }
    );

    let was_running = RT.lock().running_strategy.clone();
    kill_all_winws(true);
    ensure_tcp_timestamps();
    ensure_user_lists();

    zlog!(Info, "Проверка соединения без Zapret…");
    let base = tester::run(&targets).await;
    zlog!(Muted, "Без Zapret доступно: {}/{}", base.passed, base.total);
    if base.passed == base.total {
        zlog!(Warn, "Все адреса доступны и без Zapret — блокировок не обнаружено, результат перебора будет одинаковым.");
    }

    let mut results: Vec<Outcome> = Vec::new();
    for (i, name) in strategies.iter().enumerate() {
        if CANCEL.load(Ordering::SeqCst) {
            zlog!(Warn, "Перебор отменён пользователем.");
            kill_all_winws(false);
            break;
        }
        set_progress(i + 1, total, name);
        zlog!(Info, "");
        zlog!(Info, "Тестируется: {name}   [{} из {total}]", i + 1);
        zlog!(Info, "Запуск конфигурации...");
        if let Err(e) = spawn_strategy(name, true) {
            zlog!(Error, "Ошибка запуска: {e:#}");
            continue;
        }
        if !wait_ready().await {
            zlog!(Error, "Результат: НЕ ЗАПУСТИЛАСЬ (winws.exe завершился)");
            kill_all_winws(false);
            continue;
        }
        zlog!(Info, "Проверка соединения...");
        let score = tester::run(&targets).await;
        for f in score.failed.iter().take(6) {
            zlog!(Muted, "  ✗ {f}");
        }
        if score.failed.len() > 6 {
            zlog!(Muted, "  … и ещё {}", score.failed.len() - 6);
        }
        let verdict = if score.passed == score.total {
            ("РАБОТАЕТ", Level::Ok)
        } else if score.passed > base.passed {
            ("ЧАСТИЧНО", Level::Warn)
        } else {
            ("НЕ РАБОТАЕТ", Level::Error)
        };
        logbus::log(
            Channel::Zapret,
            verdict.1,
            format!("Результат: {} ({}/{}, ~{} мс)", verdict.0, score.passed, score.total, score.avg_ms),
        );
        zlog!(Muted, "Остановка конфигурации...");
        kill_all_winws(false);

        let perfect = score.passed == score.total;
        results.push(Outcome { name: name.clone(), score });
        if perfect && !s.zapret.full_scan {
            break;
        }
    }

    let best = results
        .iter()
        .max_by(|a, b| a.score.passed.cmp(&b.score.passed).then(b.score.avg_ms.cmp(&a.score.avg_ms)))
        .cloned();

    zlog!(Info, "");
    let Some(best) = best.filter(|b| b.score.passed > 0 && b.score.passed >= base.passed) else {
        zlog!(Error, "Рабочая конфигурация не найдена.");
        if let Some(prev) = was_running {
            zlog!(Info, "Возвращаю предыдущую конфигурацию {prev}…");
            let _ = spawn_strategy(&prev, true);
            wait_ready().await;
        }
        return Ok(None);
    };
    if best.score.passed < best.score.total {
        zlog!(Warn, "Полностью рабочей конфигурации нет — выбрана лучшая ({}/{}).", best.score.passed, best.score.total);
    }
    zlog!(Ok, "Рабочая конфигурация найдена: {}", best.name);

    let ver = local_version();
    settings::update(|st| {
        st.zapret.selected = Some(best.name.clone());
        st.zapret.selected_for_version = ver.clone();
    });

    zlog!(Info, "Запуск выбранной конфигурации...");
    spawn_strategy(&best.name, true)?;
    if wait_ready().await {
        zlog!(Ok, "Zapret успешно запущен.");
    } else {
        zlog!(Error, "winws.exe не запустился с выбранной конфигурацией.");
    }
    Ok(Some(best.name))
}

/// Действия при старте приложения: установка/обновление, перебор при необходимости, автозапуск.
pub async fn on_app_start() {
    let s = settings::get();
    let need_install = !is_installed();
    if need_install || s.check_updates_on_start {
        let res = {
            let _op = OP.lock().await;
            let _busy = BusyGuard::new("update");
            update_locked(true).await
        };
        if let Err(e) = res {
            zlog!(Error, "Обновление zapret не удалось: {e:#}");
            if !is_installed() {
                return;
            }
        }
    }

    let s = settings::get();
    let current = local_version();
    let selected_ok = s
        .zapret
        .selected
        .as_ref()
        .map(|n| dir().join(n).is_file())
        .unwrap_or(false);
    let version_changed = s.zapret.selected_for_version.is_some() && s.zapret.selected_for_version != current;

    if !selected_ok || version_changed {
        if version_changed {
            zlog!(Info, "Версия zapret изменилась — подбираю конфигурацию заново.");
        }
        let _ = search().await;
    } else if s.zapret.autostart {
        if let Err(e) = start(None).await {
            zlog!(Error, "Автозапуск: {e:#}");
        }
    }
}

pub async fn update_and_retest() -> Result<()> {
    let changed = update(true).await?;
    if changed {
        search().await?;
    }
    Ok(())
}

pub fn set_options(game_filter: Option<bool>, full_scan: Option<bool>, autostart: Option<bool>) {
    settings::update(|s| {
        if let Some(v) = game_filter {
            s.zapret.game_filter = v;
        }
        if let Some(v) = full_scan {
            s.zapret.full_scan = v;
        }
        if let Some(v) = autostart {
            s.zapret.autostart = v;
        }
    });
    logbus::status_changed();
}

pub fn select(name: String) -> Result<()> {
    if !dir().join(&name).is_file() {
        bail!("нет такой конфигурации");
    }
    let ver = local_version();
    settings::update(|s| {
        s.zapret.selected = Some(name);
        s.zapret.selected_for_version = ver;
    });
    logbus::status_changed();
    Ok(())
}

pub fn open_folder() -> Result<()> {
    winproc::shell_open(&dir().to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_handles_quotes() {
        let t = split_cmd(r#"start "zapret: general" /min "C:\a b\winws.exe" --x="C:\l l\f.txt" --y=1"#);
        assert_eq!(t, vec!["start", "zapret: general", "/min", r"C:\a b\winws.exe", r"--x=C:\l l\f.txt", "--y=1"]);
    }

    #[test]
    fn natural_order() {
        let mut v = vec!["general (ALT10).bat", "general (ALT2).bat", "general.bat", "general (ALT).bat"];
        v.sort_by(|a, b| natural_key(a).cmp(&natural_key(b)));
        assert_eq!(v, vec!["general.bat", "general (ALT).bat", "general (ALT2).bat", "general (ALT10).bat"]);
    }
}
