pub mod catalog;
pub mod claude;
pub mod codemodel;
pub mod commands;
pub mod connect;
pub mod download;
pub mod fetch;
pub mod i18n;
pub mod installs;
pub mod kit;
pub mod mihomo;
pub mod model;
pub mod runtime;
pub mod ruui;
pub mod state;
pub mod store;
pub mod sysproxy;
pub mod tray;
pub mod updates;
pub mod upkeep;
pub mod webfetch;

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
            // Только главное окно: служебное (webfetch) закрывается по-настоящему.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event
                && window.label() == "main"
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
            claude::set_flavor(&app.config().identifier);
            // Первым делом: если прошлый запуск упал с включённым системным прокси, браузеры
            // смотрят в неработающий порт — вернуть то, что было до нас.
            if let Err(exc) = sysproxy::disable() {
                eprintln!("system proxy not restored: {exc:#}");
            }
            // После create_dir_all: service_for раскрывает путь, а для этого папка должна быть.
            // Окно и команды появляются только после setup, ключей до этой строки никто не читает.
            store::use_key_service(store::key_service(
                custom.then_some(dir.as_path()),
                claude::is_dev_build(),
            ));
            store::rotate_log(&dir);
            let guard = init_logging(&dir);
            Box::leak(Box::new(guard));

            let config = store::load_or_create(&dir)?;
            i18n::apply(config.language.as_deref());
            // Прошлый выход мог не дойти до возврата модели в настройках Claude Code (ADR-010).
            codemodel::restore(&config);
            webfetch::init(app.handle());
            // Адрес для шлюза: поисковики отвечают проверкой на бота, а проходит её
            // встроенный браузер. Окно показывает себя само, если проверка просит галочку.
            if let Ok(url) = fetch::serve(true) {
                tracing::info!(address = fetch::address_of(&url), "browser fetch endpoint");
                // SAFETY: `setup` идёт до запуска потоков, которые читают окружение.
                unsafe { std::env::set_var("CTP_FETCH_URL", &url) };
                fetch::spawn_watchdog();
            } else {
                tracing::warn!("browser fetch endpoint not started");
            }
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
            // Прокси шлюза — до его автозапуска из окна.
            let proxy_on = config.proxy.is_on();
            if proxy_on && config.proxy.port != 0 {
                ctp_gateway::proxy::set(Some(format!("http://127.0.0.1:{}", config.proxy.port)));
            }
            let for_repair = config.clone();
            let check_dir = dir.clone();
            // До потоков: они обращаются к состоянию, а state() до manage() паникует.
            app.manage(AppShared::new(dir, config));
            // Упавшее само ядро (не TUN) сторож поднимает тем же путём, что при старте.
            let handle = app.handle().clone();
            mihomo::on_crash(Box::new(move || {
                commands::start_proxy_in_background(&handle)
            }));
            // Ядро — в фоне (в TUN ждёт ответа UAC).
            if proxy_on {
                let handle = app.handle().clone();
                std::thread::spawn(move || commands::start_proxy_in_background(&handle));
            }
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
                // Claude обновился, пока программа в трее, — русская копия готовится заранее.
                upkeep::spawn_copy_watch(app.handle().clone());
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
            commands::claude_state,
            commands::set_ui,
            commands::launch_claude,
            commands::launch_claude_code,
            commands::connect_check,
            commands::connect_open,
            commands::connect_apply,
            commands::connect_set_source,
            commands::connect_update,
            commands::connect_undo_open,
            commands::connect_undo_apply,
            commands::tail_log,
            commands::open_logs,
            commands::copy_text,
            commands::pick_folder,
            commands::app_exit,
            commands::check_update,
            commands::install_update,
            commands::download_code,
            commands::download_desktop,
            commands::proxy_state,
            commands::proxy_set_mode,
            commands::proxy_download_core,
            commands::proxy_add_sub,
            commands::proxy_remove_sub,
            commands::proxy_update_sub,
            commands::proxy_select,
            commands::proxy_measure,
            commands::win_minimize,
            commands::win_toggle_maximize,
            commands::win_close,
        ])
        .build(tauri::generate_context!())
        .expect("failed to start Provider Gateway for Claude")
        // Любой выход (трей, окно, конец сеанса Windows): ядро и системный прокси убрать.
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                mihomo::shutdown();
                // Ключи русской копии в реестре — назад к установленному Claude и при выходе
                // не через трей (закрыли окно без значка в трее, конец сеанса Windows).
                claude::repair_registry_on_exit(app);
                codemodel::restore_on_exit(app);
            }
        });
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
