// Без консольного окна в release-сборке.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(not(windows))]
compile_error!("ZapretTG — приложение только для Windows");

mod commands;
mod deps;
mod github;
mod logbus;
mod paths;
mod settings;
mod tester;
mod tgproxy;
mod winproc;
mod zapret;

use tauri::{RunEvent, WebviewUrl, WebviewWindowBuilder};

fn main() {
    if let Err(err) = paths::ensure_all() {
        deps::message_box(
            "ZapretTG",
            &format!(
                "Не удалось создать папку данных рядом с программой:\n{}\n\n{err}\n\nРаспакуйте приложение в папку с правом записи.",
                paths::data().display()
            ),
        );
        return;
    }

    // Окно Tauri требует WebView2, поэтому проверка компонентов — нативная, до запуска интерфейса.
    if !deps::preflight() {
        return;
    }

    let app = tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::set_check_updates,
            commands::get_logs,
            commands::clear_logs,
            commands::save_logs,
            commands::open_logs_folder,
            commands::zapret_status,
            commands::zapret_start,
            commands::zapret_stop,
            commands::zapret_restart,
            commands::zapret_search,
            commands::zapret_cancel,
            commands::zapret_update,
            commands::zapret_set_options,
            commands::zapret_select,
            commands::zapret_open_folder,
            commands::tg_status,
            commands::tg_start,
            commands::tg_stop,
            commands::tg_restart,
            commands::tg_update,
            commands::tg_set_port,
            commands::tg_regen_secret,
            commands::tg_set_autostart,
            commands::tg_open_telegram,
            commands::deps_list,
            commands::deps_install,
            commands::deps_open_all_missing,
            commands::open_url,
        ])
        .setup(|app| {
            logbus::init(app.handle().clone());

            WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title("ZapretTG")
                .inner_size(1120.0, 780.0)
                .min_inner_size(920.0, 640.0)
                .center()
                // Профиль WebView2 — внутри папки приложения, а не в %LOCALAPPDATA%.
                .data_directory(paths::webview())
                .build()?;

            tgproxy::spawn_log_tail();
            tauri::async_runtime::spawn(startup());
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("не удалось запустить интерфейс");

    app.run(|_app, event| {
        if let RunEvent::Exit = event {
            // Ничего не оставляем работать после закрытия: приложение portable.
            zapret::stop_blocking();
            tgproxy::stop_blocking();
        }
    });
}

async fn startup() {
    alog!(Info, "ZapretTG {} — папка данных: {}", env!("CARGO_PKG_VERSION"), paths::data().display());
    if !deps::is_elevated() {
        alog!(Warn, "Приложение запущено без прав администратора — zapret (WinDivert) работать не будет.");
        zlog!(Warn, "Нет прав администратора: WinDivert не сможет загрузить драйвер. Перезапустите приложение от имени администратора.");
    }
    // Даём окну подписаться на события логов.
    tokio::time::sleep(std::time::Duration::from_millis(700)).await;
    tokio::join!(zapret::on_app_start(), tgproxy::on_app_start());
}
