pub mod catalog;
pub mod claude;
pub mod commands;
pub mod i18n;
pub mod model;
pub mod runtime;
pub mod ruui;
pub mod state;
pub mod store;
pub mod tray;
pub mod updates;
pub mod upkeep;

use crate::state::AppShared;
use std::path::Path;
use tauri::Manager;
use tracing_subscriber::EnvFilter;

pub fn run() {
    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build());
    // Окно прячется в трей, и повторный запуск ярлыка поднял бы второй шлюз на том же
    // порту и конфиге — вместо этого показываем первый. Изолированный запуск
    // (CLAUDE_GATEWAY_HOME) не должен уходить в рабочий экземпляр пользователя.
    if std::env::var_os("CLAUDE_GATEWAY_HOME").is_none_or(|v| v.is_empty()) {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            tray::show_main(app);
        }));
    }
    builder
        .on_window_event(|window, event| {
            // Крестик и Alt+F4 прячут окно, шлюз работает дальше; выход — из меню трея.
            // Без значка в трее прятать некуда — тогда закрываем как обычно.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event
                && window.app_handle().tray_by_id(tray::TRAY).is_some()
            {
                api.prevent_close();
                let _ = window.hide();
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

            let config = store::load_or_create(&dir)?;
            i18n::apply(config.language.as_deref());
            fit_to_screen(app.handle());
            // После журнала и языка: предупреждение есть куда записать, меню — на нужном языке.
            if let Err(exc) = tray::build(app.handle()) {
                tracing::warn!(error = %exc, "tray icon not created, closing the window will exit");
            }
            // До автозапуска шлюза: псевдонимы моделей считаются по правилу именно
            // установленного Claude (из кеша — без чтения 50 МБ на каждом старте).
            if let Some(exe) = claude::detect(&config)
                && let Err(exc) = upkeep::apply_rules(&dir, &exe)
            {
                tracing::warn!(error = %format!("{exc:#}"), "Claude model rules");
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
            // Изолированный запуск (проверки, демо) в сеть за обновлениями не ходит.
            if !custom {
                updates::spawn_auto_check(app.handle().clone());
            }
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
            commands::pick_folder,
            commands::app_exit,
            commands::check_update,
            commands::install_update,
            commands::win_minimize,
            commands::win_toggle_maximize,
            commands::win_close,
        ])
        .run(tauri::generate_context!())
        .expect("failed to start Provider Gateway for Claude");
}

/// The default window (1200x800) is taller than the work area of small screens
/// (1366x768, or 1080p at 150%): shrink it to fit and centre it again.
fn fit_to_screen(app: &tauri::AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let (Ok(Some(monitor)), Ok(size)) = (window.current_monitor(), window.outer_size()) else {
        return;
    };
    let area = monitor.work_area().size;
    let (width, height) = fit(size.width, size.height, area.width, area.height);
    if (width, height) != (size.width, size.height) {
        let _ = window.set_size(tauri::PhysicalSize::new(width, height));
        let _ = window.center();
    }
}

fn fit(width: u32, height: u32, area_width: u32, area_height: u32) -> (u32, u32) {
    (width.min(area_width), height.min(area_height))
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

#[cfg(test)]
mod fit_tests {
    #[test]
    fn window_shrinks_only_where_the_screen_is_smaller() {
        assert_eq!(super::fit(1200, 800, 1920, 1040), (1200, 800));
        assert_eq!(super::fit(1200, 800, 1366, 728), (1200, 728));
        assert_eq!(super::fit(1500, 1000, 1280, 680), (1280, 680));
    }
}
