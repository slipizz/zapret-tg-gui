//! Живые логи: каждая строка сразу уходит в окно (событие `log`),
//! в кольцевой буфер (для перерисовки окна) и в data\logs\<канал>.log.

use std::collections::VecDeque;
use std::io::Write;

use once_cell::sync::OnceCell;
use parking_lot::Mutex;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::paths;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    App,
    Zapret,
    Tg,
}

impl Channel {
    fn file(self) -> &'static str {
        match self {
            Channel::App => "app.log",
            Channel::Zapret => "zapret.log",
            Channel::Tg => "tgproxy.log",
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Info,
    Ok,
    Warn,
    Error,
    Muted,
}

#[derive(Clone, Debug, Serialize)]
pub struct LogLine {
    pub channel: Channel,
    pub level: Level,
    pub time: String,
    pub text: String,
}

const MAX_LINES: usize = 5000;

static APP: OnceCell<AppHandle> = OnceCell::new();
static BUF: Mutex<Vec<(Channel, VecDeque<LogLine>)>> = Mutex::new(Vec::new());

pub fn init(app: AppHandle) {
    let _ = APP.set(app);
}

pub fn log(channel: Channel, level: Level, text: impl Into<String>) {
    let line = LogLine {
        channel,
        level,
        time: chrono::Local::now().format("%H:%M:%S").to_string(),
        text: text.into(),
    };

    {
        let mut buf = BUF.lock();
        let idx = match buf.iter().position(|(c, _)| *c == channel) {
            Some(i) => i,
            None => {
                buf.push((channel, VecDeque::new()));
                buf.len() - 1
            }
        };
        let q = &mut buf[idx].1;
        if q.len() >= MAX_LINES {
            q.pop_front();
        }
        q.push_back(line.clone());
    }

    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(paths::logs().join(channel.file()))
    {
        let _ = writeln!(f, "[{}] {}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"), line.text);
    }

    if let Some(app) = APP.get() {
        let _ = app.emit("log", &line);
    }
}

pub fn history(channel: Channel) -> Vec<LogLine> {
    BUF.lock()
        .iter()
        .find(|(c, _)| *c == channel)
        .map(|(_, q)| q.iter().cloned().collect())
        .unwrap_or_default()
}

pub fn clear(channel: Channel) {
    if let Some((_, q)) = BUF.lock().iter_mut().find(|(c, _)| *c == channel) {
        q.clear();
    }
}

/// Сообщить окну, что изменился статус (окно перезапросит его).
pub fn status_changed() {
    if let Some(app) = APP.get() {
        let _ = app.emit("status-changed", ());
    }
}

pub fn emit<S: Serialize + Clone>(event: &str, payload: S) {
    if let Some(app) = APP.get() {
        let _ = app.emit(event, payload);
    }
}

#[macro_export]
macro_rules! zlog {
    ($lvl:ident, $($arg:tt)*) => {
        $crate::logbus::log($crate::logbus::Channel::Zapret, $crate::logbus::Level::$lvl, format!($($arg)*))
    };
}

#[macro_export]
macro_rules! tlog {
    ($lvl:ident, $($arg:tt)*) => {
        $crate::logbus::log($crate::logbus::Channel::Tg, $crate::logbus::Level::$lvl, format!($($arg)*))
    };
}

#[macro_export]
macro_rules! alog {
    ($lvl:ident, $($arg:tt)*) => {
        $crate::logbus::log($crate::logbus::Channel::App, $crate::logbus::Level::$lvl, format!($($arg)*))
    };
}
