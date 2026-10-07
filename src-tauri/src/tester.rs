//! Проверка доступности сайтов для оценки стратегии zapret.
//!
//! Цели берутся из `utils\targets.txt` самого zapret (тот же список, что использует
//! официальный `utils\test zapret.ps1`). Для каждого https-адреса делаются два запроса:
//! с TLS 1.2 и с TLS 1.3 (как в официальном тесте). Запрос успешен, если сервер ответил
//! любым HTTP-кодом и первые 64 КБ тела дочитались без обрыва — это ловит и блокировку
//! по SNI, и «заморозку» соединения после ~16 КБ, характерную для ТСПУ.
//! Строки `PING:` пропускаются: ICMP не зависит от DPI-обхода.

use std::path::Path;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use reqwest::tls::Version;

#[derive(Debug, Clone)]
pub struct Target {
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone, Default)]
pub struct Score {
    pub passed: usize,
    pub total: usize,
    pub avg_ms: u64,
    pub failed: Vec<String>,
}

const BUILTIN: &[(&str, &str)] = &[
    ("DiscordMain", "https://discord.com"),
    ("DiscordGateway", "https://gateway.discord.gg"),
    ("DiscordCDN", "https://cdn.discordapp.com"),
    ("DiscordUpdates", "https://updates.discord.com"),
    ("YouTubeWeb", "https://www.youtube.com"),
    ("YouTubeShort", "https://youtu.be"),
    ("YouTubeImage", "https://i.ytimg.com"),
    ("YouTubeVideoRedirect", "https://redirector.googlevideo.com"),
    ("GoogleMain", "https://www.google.com"),
    ("GoogleGstatic", "https://www.gstatic.com"),
    ("CloudflareWeb", "https://www.cloudflare.com"),
    ("CloudflareCDN", "https://cdnjs.cloudflare.com"),
];

/// Формат строки: `KeyName = "https://host"`.
pub fn load_targets(file: &Path) -> Vec<Target> {
    let parsed: Vec<Target> = std::fs::read_to_string(file)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let l = l.trim();
            if l.is_empty() || l.starts_with('#') {
                return None;
            }
            let (k, v) = l.split_once('=')?;
            let v = v.trim().trim_matches('"');
            if !v.starts_with("https://") && !v.starts_with("http://") {
                return None;
            }
            Some(Target { name: k.trim().to_string(), url: v.to_string() })
        })
        .collect();
    if parsed.is_empty() {
        BUILTIN.iter().map(|(n, u)| Target { name: n.to_string(), url: u.to_string() }).collect()
    } else {
        parsed
    }
}

fn client(tls: Version) -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .no_proxy()
        .use_rustls_tls()
        .min_tls_version(tls)
        .max_tls_version(tls)
        // новое соединение на каждый запрос — иначе переиспользовались бы
        // соединения, открытые под предыдущей стратегией
        .pool_max_idle_per_host(0)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(6))
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64)")
        .build()
}

async fn check(c: reqwest::Client, url: String) -> Result<u64, String> {
    let t = Instant::now();
    let resp = c.get(&url).send().await.map_err(short_err)?;
    let mut stream = resp.bytes_stream();
    let mut got = 0usize;
    while let Some(chunk) = stream.next().await {
        got += chunk.map_err(short_err)?.len();
        if got >= 64 * 1024 {
            break;
        }
    }
    Ok(t.elapsed().as_millis() as u64)
}

fn short_err(e: reqwest::Error) -> String {
    if e.is_timeout() {
        "таймаут".into()
    } else if e.is_connect() {
        "нет соединения".into()
    } else if e.is_body() || e.is_decode() {
        "обрыв при передаче".into()
    } else {
        "ошибка TLS/HTTP".into()
    }
}

pub async fn run(targets: &[Target]) -> Score {
    let (Ok(c12), Ok(c13)) = (client(Version::TLS_1_2), client(Version::TLS_1_3)) else {
        return Score { total: targets.len() * 2, failed: vec!["не удалось создать HTTP-клиент".into()], ..Default::default() };
    };
    let mut set = tokio::task::JoinSet::new();
    for t in targets {
        for (label, c) in [("TLS1.2", c12.clone()), ("TLS1.3", c13.clone())] {
            let name = format!("{} {label}", t.name);
            let url = t.url.clone();
            set.spawn(async move { (name, check(c, url).await) });
        }
    }
    let mut score = Score { total: targets.len() * 2, ..Default::default() };
    let mut sum = 0u64;
    while let Some(r) = set.join_next().await {
        match r {
            Ok((_, Ok(ms))) => {
                score.passed += 1;
                sum += ms;
            }
            Ok((name, Err(e))) => score.failed.push(format!("{name}: {e}")),
            Err(_) => score.failed.push("внутренняя ошибка проверки".into()),
        }
    }
    score.failed.sort();
    score.avg_ms = if score.passed > 0 { sum / score.passed as u64 } else { 0 };
    score
}
