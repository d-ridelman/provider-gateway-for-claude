pub mod catalog;
pub mod claude;
pub mod commands;
pub mod model;
pub mod runtime;
pub mod ruui;
pub mod state;
pub mod store;
pub mod upkeep;

use crate::state::AppShared;
use std::path::Path;
use tauri::Manager;
use tracing_subscriber::EnvFilter;

pub fn run() {
    let mut builder = tauri::Builder::default();
    // Окно прячется в трей, и повторный запуск ярлыка поднял бы второй шлюз на том же
    // порту и конфиге — вместо этого показываем первый. Изолированный запуск
    // (CLAUDE_GATEWAY_HOME) не должен уходить в рабочий экземпляр пользователя.
    if std::env::var_os("CLAUDE_GATEWAY_HOME").is_none_or(|v| v.is_empty()) {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main(app);
        }));
    }
    builder
        .on_window_event(|window, event| {
            // Крестик и Alt+F4 прячут окно, шлюз работает дальше; выход — из меню трея.
            // Без значка в трее прятать некуда — тогда закрываем как обычно.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.app_handle().tray_by_id(TRAY).is_some() {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .setup(|app| {
            // Своя папка настроек: портативный запуск или изолированная проверка окна.
            // Ключи всё равно в Credential Manager (общем для пользователя).
            let home = std::env::var_os("CLAUDE_GATEWAY_HOME")
                .filter(|v| !v.is_empty())
                .map(std::path::PathBuf::from);
            let custom = home.is_some();
            let dir = match home {
                Some(home) => home,
                None => app.path().app_config_dir()?,
            };
            std::fs::create_dir_all(&dir)?;
            // После create_dir_all: service_for раскрывает путь, а для этого папка должна быть.
            // Окно и команды появляются только после setup, ключей до этой строки никто не читает.
            store::use_key_service(store::service_for(custom.then_some(dir.as_path())));
            store::rotate_log(&dir);
            let guard = init_logging(&dir);
            Box::leak(Box::new(guard));
            // После журнала: иначе предупреждение некуда записать.
            if let Err(exc) = build_tray(app.handle()) {
                tracing::warn!(error = %exc, "значок в трее не создан — крестик закроет программу");
            }

            let config = store::load_or_create(&dir)?;
            // До автозапуска шлюза: псевдонимы моделей считаются по правилу именно
            // установленного Claude (из кеша — без чтения 50 МБ на каждом старте).
            if let Some(exe) = claude::detect(&config) {
                if let Err(exc) = upkeep::apply_rules(&dir, &exe) {
                    tracing::warn!(error = %format!("{exc:#}"), "правило моделей Claude");
                }
            }
            let for_repair = config.clone();
            let check_dir = dir.clone();
            // До потока: он обращается к состоянию, а state() до manage() паникует.
            app.manage(AppShared::new(dir, config));
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                claude::repair_registry(&for_repair);
                // Перевод и русская копия — заранее, а не в момент «Открыть Claude».
                // Изолированный запуск (CLAUDE_GATEWAY_HOME) фоном копию не собирает.
                upkeep::check_into(&handle, &for_repair, &check_dir, !custom);
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_config,
            commands::save_provider,
            commands::delete_provider,
            commands::fetch_models,
            commands::test_provider,
            commands::save_settings,
            commands::start_gateway,
            commands::stop_gateway,
            commands::status,
            commands::launch_claude,
            commands::launch_claude_code,
            commands::tail_log,
            commands::open_logs,
            commands::copy_text,
            commands::win_minimize,
            commands::win_toggle_maximize,
            commands::win_close,
        ])
        .run(tauri::generate_context!())
        .expect("не удалось запустить Claude Provider Gateway");
}

const TRAY: &str = "main";

fn show_main(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn build_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or_else(|| tauri::Error::AssetNotFound("значок приложения".into()))?;
    let open = MenuItem::with_id(app, "open", "Открыть", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Выход (остановить шлюз)", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit])?;
    TrayIconBuilder::with_id(TRAY)
        .icon(icon)
        .tooltip("Claude Provider Gateway")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => show_main(app),
            "quit" => app.exit(0),
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

fn init_logging(dir: &Path) -> tracing_appender::non_blocking::WorkerGuard {
    let appender = tracing_appender::rolling::never(dir, "gateway.log");
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("ctp_gateway=info,ctp_app=info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(writer)
        .with_ansi(false)
        .try_init();
    guard
}
