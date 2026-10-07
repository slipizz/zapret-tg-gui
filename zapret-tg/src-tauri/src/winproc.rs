//! Работа с процессами Windows: запуск без консольного окна, поиск и завершение.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, TerminateProcess, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
};

pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn hidden(program: impl AsRef<OsStr>) -> Command {
    use std::os::windows::process::CommandExt;
    let mut c = Command::new(program);
    c.creation_flags(CREATE_NO_WINDOW);
    c
}

/// Выполнить системную команду (sc.exe, netsh.exe) скрыто и вернуть вывод.
pub fn run_hidden(program: &str, args: &[&str]) -> std::io::Result<(i32, String)> {
    let out = hidden(program).args(args).stdin(Stdio::null()).output()?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Ok((out.status.code().unwrap_or(-1), text))
}

#[derive(Debug, Clone)]
pub struct ProcInfo {
    pub pid: u32,
    pub exe: Option<PathBuf>,
}

/// Все процессы с указанным именем файла (без учёта регистра).
pub fn find_by_name(name: &str) -> Vec<ProcInfo> {
    let mut out = Vec::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return out };
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        if Process32FirstW(snap, &mut e).is_ok() {
            loop {
                let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
                let exe_name = String::from_utf16_lossy(&e.szExeFile[..len]);
                if exe_name.eq_ignore_ascii_case(name) {
                    out.push(ProcInfo { pid: e.th32ProcessID, exe: image_path(e.th32ProcessID) });
                }
                if Process32NextW(snap, &mut e).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    out
}

fn image_path(pid: u32) -> Option<PathBuf> {
    unsafe {
        let h: HANDLE = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = vec![0u16; 1024];
        let mut len = buf.len() as u32;
        let r = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(h);
        r.ok()?;
        Some(PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])))
    }
}

pub fn kill(pid: u32) -> bool {
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_TERMINATE, false, pid) else { return false };
        let ok = TerminateProcess(h, 1).is_ok();
        let _ = CloseHandle(h);
        ok
    }
}

pub fn is_alive(pid: u32) -> bool {
    unsafe {
        match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            Ok(h) => {
                let mut code = 0u32;
                let r = windows::Win32::System::Threading::GetExitCodeProcess(h, &mut code);
                let _ = CloseHandle(h);
                // STILL_ACTIVE = 259
                r.is_ok() && code == 259
            }
            Err(_) => false,
        }
    }
}

/// Процесс запущен из папки `dir` (сравнение без учёта регистра).
pub fn exe_inside(p: &ProcInfo, dir: &Path) -> bool {
    match &p.exe {
        Some(exe) => exe
            .to_string_lossy()
            .to_lowercase()
            .starts_with(&dir.to_string_lossy().to_lowercase()),
        None => false,
    }
}

/// Открыть URL / tg://-ссылку обработчиком Windows по умолчанию.
pub fn shell_open(target: &str) -> anyhow::Result<()> {
    use windows::core::HSTRING;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let r = unsafe { ShellExecuteW(None, &HSTRING::from("open"), &HSTRING::from(target), None, None, SW_SHOWNORMAL) };
    // ShellExecute возвращает значение > 32 при успехе.
    if (r.0 as isize) <= 32 {
        anyhow::bail!("Windows не нашла приложение для открытия «{target}» (код {})", r.0 as isize);
    }
    Ok(())
}
