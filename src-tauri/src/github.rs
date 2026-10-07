//! Загрузка релизов ТОЛЬКО из официальных репозиториев через GitHub API.
//! Целостность: SHA-256, который GitHub сам публикует для каждого файла релиза
//! (поле `digest` в API, оно же показывается на странице релиза).

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

pub const ZAPRET_REPO: &str = "Flowseal/zapret-discord-youtube";
pub const TGPROXY_REPO: &str = "Flowseal/tg-ws-proxy";

const UA: &str = concat!("ZapretTG/", env!("CARGO_PKG_VERSION"), " (+https://github.com)");

#[derive(Debug, Clone, Deserialize)]
pub struct Asset {
    pub name: String,
    pub browser_download_url: String,
    pub size: u64,
    /// "sha256:<hex>" — добавляется GitHub автоматически.
    #[serde(default)]
    pub digest: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Release {
    pub tag_name: String,
    #[serde(default)]
    pub assets: Vec<Asset>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Progress {
    pub what: String,
    pub done: u64,
    pub total: u64,
}

pub fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(UA)
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(600))
        .build()?)
}

pub async fn latest_release(repo: &str) -> Result<Release> {
    let url = format!("https://api.github.com/repos/{repo}/releases/latest");
    let resp = client()?
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .with_context(|| format!("нет доступа к {url}"))?;
    if !resp.status().is_success() {
        bail!("GitHub API вернул {} для {repo}", resp.status());
    }
    Ok(resp.json::<Release>().await?)
}

/// Разрешены только эти хосты (официальные адреса GitHub для файлов релизов).
fn is_official_github_url(url: &str) -> bool {
    url.starts_with("https://github.com/Flowseal/")
}

/// Скачивает файл релиза, сверяет размер и SHA-256. Возвращает путь к файлу.
pub async fn download_asset<F: Fn(Progress)>(asset: &Asset, dest_dir: &Path, on_progress: F) -> Result<PathBuf> {
    if !is_official_github_url(&asset.browser_download_url) {
        bail!("отклонён неофициальный адрес загрузки: {}", asset.browser_download_url);
    }
    tokio::fs::create_dir_all(dest_dir).await?;
    let dest = dest_dir.join(&asset.name);
    let tmp = dest.with_extension("part");

    let resp = client()?.get(&asset.browser_download_url).send().await?;
    if !resp.status().is_success() {
        bail!("загрузка {} завершилась с кодом {}", asset.name, resp.status());
    }
    let total = resp.content_length().unwrap_or(asset.size);
    let mut file = tokio::fs::File::create(&tmp).await?;
    let mut hasher = Sha256::new();
    let mut done = 0u64;
    let mut stream = resp.bytes_stream();
    let mut last_emit = 0u64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        hasher.update(&chunk);
        file.write_all(&chunk).await?;
        done += chunk.len() as u64;
        if done - last_emit > 256 * 1024 || done == total {
            last_emit = done;
            on_progress(Progress { what: asset.name.clone(), done, total });
        }
    }
    file.flush().await?;
    drop(file);

    if asset.size > 0 && done != asset.size {
        let _ = tokio::fs::remove_file(&tmp).await;
        bail!("размер {} не совпал: ожидалось {} байт, получено {}", asset.name, asset.size, done);
    }

    let actual = hex::encode(hasher.finalize());
    match asset.digest.as_deref().and_then(|d| d.strip_prefix("sha256:")) {
        Some(expected) if !expected.eq_ignore_ascii_case(&actual) => {
            let _ = tokio::fs::remove_file(&tmp).await;
            bail!("SHA-256 не совпал для {}: ожидался {expected}, получен {actual}", asset.name);
        }
        _ => {}
    }

    if dest.exists() {
        tokio::fs::remove_file(&dest).await.ok();
    }
    tokio::fs::rename(&tmp, &dest).await?;
    Ok(dest)
}

/// Возвращает строку для лога о результате проверки хеша.
pub fn digest_note(asset: &Asset) -> String {
    match asset.digest.as_deref().and_then(|d| d.strip_prefix("sha256:")) {
        Some(h) => format!("SHA-256 совпадает с опубликованным GitHub: {}…", &h[..16.min(h.len())]),
        None => "GitHub не опубликовал SHA-256 для этого файла — проверены размер и целостность архива".into(),
    }
}

pub fn find_asset<'a>(rel: &'a Release, pred: impl Fn(&str) -> bool) -> Result<&'a Asset> {
    rel.assets
        .iter()
        .find(|a| pred(&a.name))
        .ok_or_else(|| anyhow!("в релизе {} нет подходящего файла", rel.tag_name))
}

/// "v1.11.1" и "1.11.1" считаются одной версией.
pub fn same_version(a: &str, b: &str) -> bool {
    a.trim_start_matches(['v', 'V']) == b.trim_start_matches(['v', 'V'])
}
