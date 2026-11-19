use crate::model::{
    FetchedModels, ProbeInput, SaveProviderInput, SettingsInput, UiConfig, UiStatus,
};
use crate::state::AppShared;
use crate::{claude, runtime, store};
use std::process::Command;
use tauri::State;

#[tauri::command]
pub fn get_config(shared: State<'_, AppShared>) -> Result<UiConfig, String> {
    let config = shared.config.lock().unwrap().clone();
    Ok(store::ui_config(&config, &shared.config_dir))
}

#[tauri::command]
pub fn save_provider(
    shared: State<'_, AppShared>,
    input: SaveProviderInput,
) -> Result<UiConfig, String> {
    let row = store::clean_row(input.provider)?;
    let original = input.original_id.as_deref();
    let renamed_from = original.filter(|old| *old != row.id);
    let mut config = shared.config.lock().unwrap();
    let next = store::apply_provider(&config, row.clone(), original)?;

    // ID, под которым в конфиге раньше никого не было: новый провайдер или переименование.
    let fresh_id = original.is_none() || renamed_from.is_some();
    let typed = input.api_key.as_deref().map(str::trim).unwrap_or("");
    let new_key = if input.clear_key {
        None
    } else if !typed.is_empty() {
        Some(typed.to_string())
    } else if let Some(old) = renamed_from {
        // Ошибка чтения прерывает сохранение: иначе ниже удалили бы единственную копию.
        store::read_key(old)?
    } else {
        None
    };

    // Старый ключ этого же ID — чтобы вернуть его, если конфиг не запишется.
    let previous = if new_key.is_some() && !fresh_id {
        store::read_key(&row.id)?
    } else {
        None
    };
    // Ключ пишем до конфига: если Credential Manager откажет, на диске ничего не поменяется.
    if let Some(key) = &new_key {
        store::set_key(&row.id, key)?;
    }
    if let Err(exc) = store::save(&shared.config_dir, &next) {
        if new_key.is_some() {
            match &previous {
                Some(old) => {
                    if let Err(undo) = store::set_key(&row.id, old) {
                        tracing::error!(provider = %row.id, error = %undo, "не удалось вернуть прежний ключ");
                        return Err(format!(
                            "Настройки не сохранены ({exc}), и прежний ключ вернуть не удалось — введите ключ заново"
                        ));
                    }
                }
                None => store::delete_key(&row.id),
            }
        }
        return Err(exc.to_string());
    }
    // Осиротевший ключ под новым ID (от прошлого сбоя) не должен достаться чужому провайдеру.
    if new_key.is_none() && (input.clear_key || fresh_id) {
        store::delete_key(&row.id);
    }
    if let Some(old) = renamed_from {
        store::delete_key(old);
    }
    *config = next;
    let ui = store::ui_config(&config, &shared.config_dir);
    drop(config);
    runtime::stop(&shared);
    Ok(ui)
}

/// Провайдер из несохранённой формы. `api_key`: `None` — сохранённый ключ этого
/// провайдера, `Some("")` — без ключа («Удалить ключ»), иначе введённый.
/// Для новой формы сохранённый ключ не берём: ID в ней набран руками и может совпасть
/// с чужим провайдером, тогда его ключ ушёл бы на другой хост.
fn probe_provider(input: ProbeInput) -> Result<ctp_core::config::ProviderConfig, String> {
    let row = store::clean_endpoint(input.provider)?;
    let key = match input.api_key {
        Some(key) => Some(key.trim().to_string()),
        None => input.original_id.as_deref().and_then(store::get_key),
    };
    Ok(store::provider_config(&row, key))
}

const CATALOG_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

#[tauri::command]
pub async fn fetch_models(input: ProbeInput) -> Result<FetchedModels, String> {
    let config = probe_provider(input)?;
    let provider = ctp_gateway::state::Provider::from_config(&config).map_err(|e| e.to_string())?;
    // Каталог OpenCode — параллельно и без права сорвать или затянуть загрузку списка:
    // после ответа провайдера ждём его не дольше CATALOG_GRACE.
    let catalog = tokio::spawn(crate::catalog::load());
    let ids = ctp_gateway::probe::list_models(&provider).await?;
    let hints = match tokio::time::timeout(CATALOG_GRACE, catalog).await {
        Ok(Ok(Some(catalog))) => crate::catalog::hints(&catalog, &config.base_url, &ids),
        _ => Default::default(),
    };
    Ok(FetchedModels {
        ids,
        catalog: hints,
    })
}

/// Короткий настоящий запрос к первой модели формы через шлюз.
#[tauri::command]
pub async fn test_provider(input: ProbeInput) -> Result<String, String> {
    let config = probe_provider(input)?;
    let (alias, model) = config
        .models
        .first()
        .cloned()
        .ok_or_else(|| "Добавьте хотя бы одну модель".to_string())?;
    let file = ctp_core::config::ProvidersFile {
        default: Some(alias.clone()),
        providers: vec![config],
    };
    let started = std::time::Instant::now();
    let reply = ctp_gateway::probe::chat_check(file, &alias).await?;
    Ok(format!(
        "{model} ответила за {:.1} с: «{reply}»",
        started.elapsed().as_secs_f32()
    ))
}

#[tauri::command]
pub fn delete_provider(shared: State<'_, AppShared>, id: String) -> Result<UiConfig, String> {
    let mut config = shared.config.lock().unwrap();
    config.providers.retain(|provider| provider.id != id);
    store::save(&shared.config_dir, &config).map_err(|exc| exc.to_string())?;
    store::delete_key(&id);
    let ui = store::ui_config(&config, &shared.config_dir);
    drop(config);
    runtime::stop(&shared);
    Ok(ui)
}

#[tauri::command]
pub fn save_settings(
    app: tauri::AppHandle,
    shared: State<'_, AppShared>,
    input: SettingsInput,
) -> Result<UiConfig, String> {
    let port = if input.port == 0 {
        crate::model::DEFAULT_PORT
    } else {
        input.port
    };
    let mut config = shared.config.lock().unwrap();
    // Маршруты шлюза зависят только от порта: остальные настройки его не останавливают.
    let port_changed = config.port != port;
    config.port = port;
    config.claude_path = input
        .claude_path
        .map(|path| path.trim().to_string())
        .filter(|path| !path.is_empty());
    config.russian_ui = input.russian_ui;
    config.code_dir = input
        .code_dir
        .map(|dir| dir.trim().to_string())
        .filter(|dir| !dir.is_empty());
    config.language = input
        .language
        .map(|lang| lang.trim().to_string())
        .filter(|lang| lang == "ru" || lang == "en");
    crate::i18n::apply(config.language.as_deref());
    crate::tray::relabel(&app);
    store::save(&shared.config_dir, &config).map_err(|exc| exc.to_string())?;
    let ui = store::ui_config(&config, &shared.config_dir);
    drop(config);
    if port_changed {
        runtime::stop(&shared);
    }
    Ok(ui)
}

#[tauri::command]
pub async fn start_gateway(shared: State<'_, AppShared>) -> Result<UiStatus, String> {
    runtime::start(&shared).await
}

#[tauri::command]
pub fn stop_gateway(shared: State<'_, AppShared>) -> UiStatus {
    runtime::stop(&shared);
    runtime::status(&shared)
}

#[tauri::command]
pub fn status(shared: State<'_, AppShared>) -> UiStatus {
    runtime::status(&shared)
}

/// Первая сборка русской копии копирует ~700 МБ — не в потоке окна.
#[tauri::command]
pub async fn launch_claude(
    app: tauri::AppHandle,
    shared: State<'_, AppShared>,
) -> Result<String, String> {
    let config = shared.config.lock().unwrap().clone();
    let dir = shared.config_dir.clone();
    // Программа живёт в трее сутками: Claude мог обновиться с прошлой проверки.
    let (prep_config, prep_dir) = (config.clone(), dir.clone());
    let rules_changed = tauri::async_runtime::spawn_blocking(move || {
        crate::upkeep::prepare(&prep_config, &prep_dir)
    })
    .await
    .map_err(|exc| exc.to_string())?;
    let mut status = runtime::status(&shared);
    if rules_changed && status.running {
        // Новые правила Claude — новые псевдонимы: шлюз и конфиг Claude из одного набора.
        status = runtime::start(&shared).await?;
    }
    let (launch_config, launch_dir) = (config.clone(), dir.clone());
    let result = tauri::async_runtime::spawn_blocking(move || {
        claude::launch(&launch_config, &status, &launch_dir)
    })
    .await
    .map_err(|exc| exc.to_string())?;
    // Самопроверка — заново, копию launch уже собрал.
    std::thread::spawn(move || crate::upkeep::check_into(&app, &config, &dir, false));
    result
}

#[tauri::command]
pub async fn launch_claude_code(shared: State<'_, AppShared>) -> Result<String, String> {
    let config = shared.config.lock().unwrap().clone();
    let status = runtime::status(&shared);
    // Обход PATH (сетевые диски) — не в главном потоке окна.
    tauri::async_runtime::spawn_blocking(move || claude::launch_code(&config, &status))
        .await
        .map_err(|exc| exc.to_string())?
}

#[tauri::command]
pub fn tail_log(shared: State<'_, AppShared>) -> String {
    store::tail_log(&shared.config_dir, 250)
}

#[tauri::command]
pub fn open_logs(shared: State<'_, AppShared>) -> Result<(), String> {
    Command::new("explorer")
        .arg(&shared.config_dir)
        .spawn()
        .map(|_| ())
        .map_err(|exc| exc.to_string())
}

#[tauri::command]
pub fn copy_text(text: String) -> Result<(), String> {
    let mut clipboard = arboard::Clipboard::new().map_err(|exc| exc.to_string())?;
    clipboard.set_text(text).map_err(|exc| exc.to_string())
}

#[tauri::command]
pub fn win_minimize(window: tauri::Window) -> Result<(), String> {
    window.minimize().map_err(|exc| exc.to_string())
}

#[tauri::command]
pub fn win_toggle_maximize(window: tauri::Window) -> Result<(), String> {
    let result = if window.is_maximized().unwrap_or(false) {
        window.unmaximize()
    } else {
        window.maximize()
    };
    result.map_err(|exc| exc.to_string())
}

#[tauri::command]
pub fn win_close(window: tauri::Window) -> Result<(), String> {
    window.close().map_err(|exc| exc.to_string())
}

/// Выбор папки системным диалогом; `None` — отменили.
#[tauri::command]
pub async fn pick_folder(app: tauri::AppHandle, start: Option<String>) -> Option<String> {
    use tauri_plugin_dialog::DialogExt;
    let mut dialog = app.dialog().file();
    if let Some(start) = start.filter(|s| std::path::Path::new(s).is_dir()) {
        dialog = dialog.set_directory(start);
    }
    // blocking_* нельзя в главном потоке; async-команда идёт в пуле Tauri.
    tauri::async_runtime::spawn_blocking(move || dialog.blocking_pick_folder())
        .await
        .ok()
        .flatten()
        .and_then(|path| path.into_path().ok())
        .map(|path| path.display().to_string())
}

/// Полный выход (как «Выход» в трее): шлюз останавливается вместе с программой.
#[tauri::command]
pub fn app_exit(app: tauri::AppHandle) {
    app.exit(0);
}
