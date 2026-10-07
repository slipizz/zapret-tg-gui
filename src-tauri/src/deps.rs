//! Проверка и (только с согласия пользователя) установка системных компонентов.
//!
//! Что реально нужно приложению:
//! * **Microsoft Edge WebView2 Runtime** — движок окна (Tauri). Обязателен.
//!   В Windows 11 есть всегда, в Windows 10 — почти всегда.
//! * **Visual C++ Redistributable** — НЕ нужен: exe собран со статическим CRT,
//!   winws.exe (zapret) использует cygwin1.dll из своего архива, а
//!   TgWsProxy_windows.exe (PyInstaller) несёт среду выполнения в себе.
//!   Компонент показывается в списке как информационный.
//! * **.NET Runtime** — не используется вовсе.
//!
//! Загрузка — только с официальных адресов Microsoft; перед запуском у установщика
//! проверяется цифровая подпись Authenticode и то, что подписант — Microsoft Corporation.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use windows::core::{HSTRING, PCWSTR};
use windows::Win32::System::Registry::{RegGetValueW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ};

use crate::{paths, winproc};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Id {
    WebView2,
    VcRedist,
}

#[derive(Debug, Clone, Serialize)]
pub struct Component {
    pub id: Id,
    pub name: &'static str,
    pub required: bool,
    pub why: &'static str,
    /// Официальная страница загрузки (для ручной установки).
    pub page_url: &'static str,
    /// Официальная прямая ссылка на установщик (для автоматической установки).
    pub download_url: &'static str,
    pub installer_name: &'static str,
    pub silent_args: &'static [&'static str],
    pub installed: bool,
    pub version: Option<String>,
}

const WEBVIEW2_GUID: &str = "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";

fn reg_string(root: HKEY, subkey: &str, value: &str) -> Option<String> {
    let mut buf = vec![0u16; 256];
    let mut size = (buf.len() * 2) as u32;
    let r = unsafe {
        RegGetValueW(
            root,
            &HSTRING::from(subkey),
            &HSTRING::from(value),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if r.is_err() {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..len]))
}

fn reg_dword(root: HKEY, subkey: &str, value: &str) -> Option<u32> {
    let mut v = 0u32;
    let mut size = 4u32;
    let r = unsafe {
        RegGetValueW(
            root,
            &HSTRING::from(subkey),
            &HSTRING::from(value),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut v as *mut u32).cast()),
            Some(&mut size),
        )
    };
    r.is_ok().then_some(v)
}

/// Способ обнаружения WebView2, рекомендованный Microsoft: значение `pv` в ключе EdgeUpdate.
fn webview2_version() -> Option<String> {
    let keys = [
        (HKEY_LOCAL_MACHINE, format!(r"SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{WEBVIEW2_GUID}")),
        (HKEY_LOCAL_MACHINE, format!(r"SOFTWARE\Microsoft\EdgeUpdate\Clients\{WEBVIEW2_GUID}")),
        (HKEY_CURRENT_USER, format!(r"Software\Microsoft\EdgeUpdate\Clients\{WEBVIEW2_GUID}")),
    ];
    keys.iter()
        .filter_map(|(root, k)| reg_string(*root, k, "pv"))
        .find(|v| !v.is_empty() && v != "0.0.0.0")
}

fn vcredist_version() -> Option<String> {
    let arch = if cfg!(target_arch = "aarch64") { "arm64" } else { "x64" };
    for k in [
        format!(r"SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\{arch}"),
        format!(r"SOFTWARE\WOW6432Node\Microsoft\VisualStudio\14.0\VC\Runtimes\{arch}"),
    ] {
        if reg_dword(HKEY_LOCAL_MACHINE, &k, "Installed") == Some(1) {
            return reg_string(HKEY_LOCAL_MACHINE, &k, "Version").or(Some("установлен".into()));
        }
    }
    None
}

pub fn all() -> Vec<Component> {
    let wv = webview2_version();
    let vc = vcredist_version();
    let vc_url = if cfg!(target_arch = "aarch64") {
        "https://aka.ms/vs/17/release/vc_redist.arm64.exe"
    } else {
        "https://aka.ms/vs/17/release/vc_redist.x64.exe"
    };
    vec![
        Component {
            id: Id::WebView2,
            name: "Microsoft Edge WebView2 Runtime",
            required: true,
            why: "Отрисовывает окно приложения",
            page_url: "https://developer.microsoft.com/microsoft-edge/webview2/",
            download_url: "https://go.microsoft.com/fwlink/p/?LinkId=2124703",
            installer_name: "MicrosoftEdgeWebview2Setup.exe",
            silent_args: &["/silent", "/install"],
            installed: wv.is_some(),
            version: wv,
        },
        Component {
            id: Id::VcRedist,
            name: "Microsoft Visual C++ Redistributable 2015–2022",
            required: false,
            why: "Не требуется: приложение собрано со статической CRT",
            page_url: "https://learn.microsoft.com/cpp/windows/latest-supported-vc-redist",
            download_url: vc_url,
            installer_name: "vc_redist.exe",
            silent_args: &["/install", "/quiet", "/norestart"],
            installed: vc.is_some(),
            version: vc,
        },
    ]
}

/// Разрешаем только адреса Microsoft (включая конечный адрес после редиректов).
fn is_microsoft_host(url: &reqwest::Url) -> bool {
    url.scheme() == "https"
        && url
            .host_str()
            .map(|h| h == "microsoft.com" || h.ends_with(".microsoft.com") || h == "aka.ms")
            .unwrap_or(false)
}

/// Скачать официальный установщик, проверить подпись, запустить и дождаться завершения.
pub async fn install<F: Fn(String)>(c: &Component, log: F) -> Result<()> {
    let start = reqwest::Url::parse(c.download_url)?;
    if !is_microsoft_host(&start) {
        bail!("адрес {} не принадлежит Microsoft", c.download_url);
    }
    log(format!("Загрузка {} с {}", c.name, c.download_url));
    let client = crate::github::client()?;
    let resp = client.get(start).send().await?;
    let final_url = resp.url().clone();
    if !is_microsoft_host(&final_url) {
        bail!("переадресация на неофициальный адрес отклонена: {final_url}");
    }
    log(format!("Фактический источник: {final_url}"));
    if !resp.status().is_success() {
        bail!("сервер вернул {}", resp.status());
    }
    let bytes = resp.bytes().await?;
    let path: PathBuf = paths::downloads().join(c.installer_name);
    std::fs::create_dir_all(paths::downloads())?;
    std::fs::write(&path, &bytes)?;
    log(format!("Загружено {:.1} МБ", bytes.len() as f64 / 1_048_576.0));

    let signer = verify_authenticode(&path).context("проверка цифровой подписи")?;
    if !signer.contains("Microsoft Corporation") {
        let _ = std::fs::remove_file(&path);
        bail!("файл подписан «{signer}», а не Microsoft Corporation — запуск отменён");
    }
    log(format!("Подпись проверена: {signer}"));

    log("Запуск официального установщика…".into());
    let args: Vec<String> = c.silent_args.iter().map(|s| s.to_string()).collect();
    let p = path.clone();
    let code = tokio::task::spawn_blocking(move || std::process::Command::new(&p).args(&args).status())
        .await??
        .code()
        .unwrap_or(-1);
    let _ = std::fs::remove_file(&path);
    // 1638 — уже установлена более новая версия, 3010 — нужна перезагрузка.
    if !matches!(code, 0 | 1638 | 3010) {
        bail!("установщик завершился с кодом {code}");
    }
    if code == 3010 {
        log("Установщик просит перезагрузить компьютер".into());
    }
    log("Установка завершена".into());
    Ok(())
}

/// WinVerifyTrust + имя подписанта из сертификата.
pub fn verify_authenticode(path: &Path) -> Result<String> {
    use std::ffi::c_void;
    use windows::Win32::Foundation::{HANDLE, HWND};
    use windows::Win32::Security::Cryptography::{CertGetNameStringW, CERT_NAME_SIMPLE_DISPLAY_TYPE};
    use windows::Win32::Security::WinTrust::{
        WTHelperGetProvSignerFromChain, WTHelperProvDataFromStateData, WinVerifyTrust, WINTRUST_ACTION_GENERIC_VERIFY_V2,
        WINTRUST_DATA, WINTRUST_DATA_0, WINTRUST_FILE_INFO, WTD_CHOICE_FILE, WTD_REVOKE_NONE, WTD_STATEACTION_CLOSE,
        WTD_STATEACTION_VERIFY, WTD_UI_NONE,
    };

    let wpath = HSTRING::from(path.as_os_str());
    unsafe {
        let mut file = WINTRUST_FILE_INFO {
            cbStruct: std::mem::size_of::<WINTRUST_FILE_INFO>() as u32,
            pcwszFilePath: PCWSTR(wpath.as_ptr()),
            hFile: HANDLE::default(),
            pgKnownSubject: std::ptr::null_mut(),
        };
        let mut data = WINTRUST_DATA {
            cbStruct: std::mem::size_of::<WINTRUST_DATA>() as u32,
            dwUIChoice: WTD_UI_NONE,
            fdwRevocationChecks: WTD_REVOKE_NONE,
            dwUnionChoice: WTD_CHOICE_FILE,
            Anonymous: WINTRUST_DATA_0 { pFile: &mut file },
            dwStateAction: WTD_STATEACTION_VERIFY,
            ..Default::default()
        };
        let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
        let status = WinVerifyTrust(HWND(-1isize as *mut c_void), &mut action, &mut data as *mut _ as *mut c_void);

        let mut signer = String::new();
        if status == 0 {
            let prov = WTHelperProvDataFromStateData(data.hWVTStateData);
            if !prov.is_null() {
                let sgnr = WTHelperGetProvSignerFromChain(prov, 0, false, 0);
                if !sgnr.is_null() && (*sgnr).csCertChain > 0 && !(*sgnr).pasCertChain.is_null() {
                    let cert = (*(*sgnr).pasCertChain).pCert;
                    let mut buf = vec![0u16; 256];
                    let n = CertGetNameStringW(cert, CERT_NAME_SIMPLE_DISPLAY_TYPE, 0, None, Some(&mut buf));
                    if n > 1 {
                        signer = String::from_utf16_lossy(&buf[..(n as usize - 1)]);
                    }
                }
            }
        }

        data.dwStateAction = WTD_STATEACTION_CLOSE;
        WinVerifyTrust(HWND(-1isize as *mut c_void), &mut action, &mut data as *mut _ as *mut c_void);

        if status != 0 {
            bail!("подпись недействительна (код 0x{:08X})", status as u32);
        }
        if signer.is_empty() {
            bail!("не удалось прочитать подписанта");
        }
        Ok(signer)
    }
}

// ───────────── Нативное окно до запуска интерфейса (WebView2 может отсутствовать) ─────────────

const BTN_AUTO: i32 = 101;
const BTN_PAGE: i32 = 102;
const BTN_RECHECK: i32 = 103;

/// Возвращает true, если можно запускать интерфейс.
pub fn preflight() -> bool {
    loop {
        let comps = all();
        let missing: Vec<&Component> = comps.iter().filter(|c| c.required && !c.installed).collect();
        if missing.is_empty() {
            return true;
        }

        let mut content = String::from("Необходимые компоненты:\n\n");
        for c in comps.iter().filter(|c| c.required) {
            content.push_str(&format!(
                "{} {}\n      {}\n",
                if c.installed { "✓" } else { "✗" },
                c.name,
                if c.installed { "установлен" } else { "отсутствует в системе" }
            ));
        }
        content.push_str("\nВыберите способ установки.");

        let sources: Vec<String> = missing.iter().map(|c| c.download_url.to_string()).collect();
        let auto_text = format!("Скачать и установить автоматически\nИсточник: {}", sources.join(", "));
        let page_text = if missing.len() > 1 { "Открыть все официальные страницы" } else { "Открыть официальную страницу" };
        let page_text = format!("{page_text}\nУстановите вручную, затем нажмите «Проверить снова»");

        let choice = task_dialog(
            "ZapretTG — необходим дополнительный компонент",
            "Для работы приложения требуется дополнительный компонент",
            &content,
            &[(BTN_AUTO, &auto_text), (BTN_PAGE, &page_text), (BTN_RECHECK, "Проверить снова")],
        );

        match choice {
            BTN_AUTO => {
                let owned: Vec<Component> = missing.iter().map(|c| (*c).clone()).collect();
                for c in owned {
                    let res = tauri::async_runtime::block_on(install(&c, |m| eprintln!("{m}")));
                    if let Err(e) = res {
                        message_box(
                            "ZapretTG — ошибка установки",
                            &format!("Не удалось установить {}:\n{e:#}\n\nВы можете установить компонент вручную со страницы:\n{}", c.name, c.page_url),
                        );
                    }
                }
            }
            BTN_PAGE => {
                for c in &missing {
                    let _ = winproc::shell_open(c.page_url);
                }
            }
            BTN_RECHECK => {}
            _ => return false,
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// TaskDialog с командными кнопками. Возвращает id нажатой кнопки (IDCANCEL = 2).
fn task_dialog(title: &str, instruction: &str, content: &str, buttons: &[(i32, &str)]) -> i32 {
    use windows::Win32::UI::Controls::{
        TaskDialogIndirect, TASKDIALOGCONFIG, TASKDIALOG_BUTTON, TDCBF_CANCEL_BUTTON, TDF_ALLOW_DIALOG_CANCELLATION,
        TDF_USE_COMMAND_LINKS,
    };

    let title_w = wide(title);
    let instr_w = wide(instruction);
    let content_w = wide(content);
    let texts: Vec<Vec<u16>> = buttons.iter().map(|(_, t)| wide(t)).collect();
    let btns: Vec<TASKDIALOG_BUTTON> = buttons
        .iter()
        .zip(texts.iter())
        .map(|((id, _), t)| TASKDIALOG_BUTTON { nButtonID: *id, pszButtonText: PCWSTR(t.as_ptr()) })
        .collect();

    let cfg = TASKDIALOGCONFIG {
        cbSize: std::mem::size_of::<TASKDIALOGCONFIG>() as u32,
        dwFlags: TDF_USE_COMMAND_LINKS | TDF_ALLOW_DIALOG_CANCELLATION,
        dwCommonButtons: TDCBF_CANCEL_BUTTON,
        pszWindowTitle: PCWSTR(title_w.as_ptr()),
        pszMainInstruction: PCWSTR(instr_w.as_ptr()),
        pszContent: PCWSTR(content_w.as_ptr()),
        cButtons: btns.len() as u32,
        pButtons: btns.as_ptr(),
        nDefaultButton: buttons.first().map(|b| b.0).unwrap_or(0),
        ..Default::default()
    };
    let mut pressed = 0i32;
    let ok = unsafe { TaskDialogIndirect(&cfg, Some(&mut pressed), None, None) };
    if ok.is_ok() {
        return pressed;
    }

    // Запасной вариант без comctl32 v6.
    let text = format!(
        "{instruction}\n\n{content}\n\nДа — скачать и установить автоматически\nНет — открыть официальную страницу\nОтмена — выйти"
    );
    match message_box_ex(title, &text, true) {
        6 => BTN_AUTO,
        7 => BTN_PAGE,
        _ => 2,
    }
}

pub fn message_box(title: &str, text: &str) {
    message_box_ex(title, text, false);
}

fn message_box_ex(title: &str, text: &str, yes_no_cancel: bool) -> i32 {
    use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONINFORMATION, MB_ICONWARNING, MB_OK, MB_YESNOCANCEL};
    let flags = if yes_no_cancel { MB_YESNOCANCEL | MB_ICONWARNING } else { MB_OK | MB_ICONINFORMATION };
    unsafe { MessageBoxW(None, &HSTRING::from(text), &HSTRING::from(title), flags).0 }
}

pub fn is_elevated() -> bool {
    unsafe { windows::Win32::UI::Shell::IsUserAnAdmin().as_bool() }
}
