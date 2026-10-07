fn main() {
    // Свой манифест: запрос прав администратора (WinDivert, используемый zapret,
    // загружает драйвер и без прав администратора не работает) + Common Controls v6
    // (нужны для TaskDialog в окне проверки компонентов и для диалогов Tauri).
    let windows = tauri_build::WindowsAttributes::new().app_manifest(include_str!("app.manifest"));
    tauri_build::try_build(tauri_build::Attributes::new().windows_attributes(windows))
        .expect("failed to run tauri-build");
}
