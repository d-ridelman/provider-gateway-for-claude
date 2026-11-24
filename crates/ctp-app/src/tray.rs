//! Tray icon: the window hides here, the gateway keeps running; Exit stops it.

use crate::tr;
use tauri::Manager;
use tauri::menu::{Menu, MenuItem};

pub const TRAY: &str = "main";

pub fn show_main(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn menu(app: &tauri::AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let open = MenuItem::with_id(app, "open", tr!("Открыть", "Open"), true, None::<&str>)?;
    let quit = MenuItem::with_id(
        app,
        "quit",
        tr!("Выход (остановить шлюз)", "Exit (stop the gateway)"),
        true,
        None::<&str>,
    )?;
    Menu::with_items(app, &[&open, &quit])
}

pub fn build(app: &tauri::AppHandle) -> tauri::Result<()> {
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    // The tray's own 32 px icon (tools/make_icon.py): the window icon would hand it the 256 px frame to shrink.
    TrayIconBuilder::with_id(TRAY)
        .icon(tauri::include_image!("icons/tray.png"))
        .tooltip("Provider Gateway for Claude")
        .menu(&menu(app)?)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => show_main(app),
            "quit" => {
                crate::mihomo::shutdown();
                crate::claude::repair_registry_on_exit(app);
                app.exit(0)
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

/// Menu labels in the current language (after the user switched it).
pub fn relabel(app: &tauri::AppHandle) {
    if let (Some(tray), Ok(menu)) = (app.tray_by_id(TRAY), menu(app)) {
        let _ = tray.set_menu(Some(menu));
    }
}
