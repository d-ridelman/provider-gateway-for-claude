use crate::model::{
    Config, FetchedModels, ProbeInput, SaveProviderInput, SettingsInput, UiConfig, UiStatus,
};
use crate::state::AppShared;
use crate::{claude, runtime, store, tr};
use std::process::Command;
use tauri::State;

#[tauri::command(async)]
pub fn get_config(shared: State<'_, AppShared>) -> Result<UiConfig, String> {
    let config = shared.config.lock().unwrap().clone();
    Ok(store::ui_config(&config, &shared.config_dir))
}

#[tauri::command(async)]
pub fn save_provider(
    shared: State<'_, AppShared>,
    input: SaveProviderInput,
) -> Result<UiConfig, String> {
    save_provider_in(&shared, input)
}

/// Ядро `save_provider` без Tauri: ключ и конфиг пишутся в два шага, и тесты ходят сюда.
fn save_provider_in(shared: &AppShared, input: SaveProviderInput) -> Result<UiConfig, String> {
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
                        tracing::error!(provider = %row.id, error = %undo, "failed to restore the previous key");
                        return Err(tr!(
                            "Настройки не сохранены ({exc}), и прежний ключ вернуть не удалось — введите ключ заново",
                            "Settings not saved ({exc}) and the previous key could not be restored; enter the key again"
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
    // У новой модели появился псевдоним: окно сжатия пишется и ему (ADR-019).
    crate::compact::sync(&config, None);
    let ui = store::ui_config(&config, &shared.config_dir);
    drop(config);
    runtime::stop(shared);
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
        .ok_or_else(|| tr!("Добавьте хотя бы одну модель", "Add at least one model"))?;
    let file = ctp_core::config::ProvidersFile {
        default: Some(alias.clone()),
        providers: vec![config],
    };
    let started = std::time::Instant::now();
    let reply = ctp_gateway::probe::chat_check(file, &alias).await?;
    let secs = started.elapsed().as_secs_f32();
    Ok(tr!(
        "{model} ответила за {secs:.1} с: «{reply}»",
        "{model} replied in {secs:.1} s: \"{reply}\""
    ))
}

#[tauri::command(async)]
pub fn delete_provider(shared: State<'_, AppShared>, id: String) -> Result<UiConfig, String> {
    delete_provider_in(&shared, &id)
}

fn delete_provider_in(shared: &AppShared, id: &str) -> Result<UiConfig, String> {
    let mut config = shared.config.lock().unwrap();
    let mut next = config.clone();
    next.providers.retain(|provider| provider.id != id);
    // Disk first: if the write fails, memory and the key stay as they were.
    store::save(&shared.config_dir, &next).map_err(|exc| exc.to_string())?;
    *config = next;
    store::delete_key(id);
    let ui = store::ui_config(&config, &shared.config_dir);
    drop(config);
    runtime::stop(shared);
    Ok(ui)
}

#[tauri::command(async)]
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
    let compact_window = match input
        .compact_window
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        None => None,
        Some(text) if crate::compact::parse(text).is_some() => Some(text.to_string()),
        Some(_) => {
            return Err(tr!(
                "Окно сжатия: 400k, 400000 или 40% (не меньше 100k)",
                "Compaction window: 400k, 400000 or 40% (at least 100k)"
            ));
        }
    };
    let mut config = shared.config.lock().unwrap();
    // Шлюз останавливают только порт и роли моделей (основная — модель по умолчанию).
    let port_changed = config.port != port;
    // Сначала на диск, потом в память: при ошибке записи всё остаётся как было.
    let mut next = config.clone();
    next.port = port;
    next.claude_path = input
        .claude_path
        .map(|path| path.trim().to_string())
        .filter(|path| !path.is_empty());
    next.russian_ui = input.russian_ui;
    next.builtin_browser = input.builtin_browser;
    next.artifacts = input.artifacts;
    next.cowork_mode = pick(Some(input.cowork_mode), crate::model::COWORK_MODES)
        .unwrap_or_else(|| "host".to_string());
    next.code_dir = input
        .code_dir
        .map(|dir| dir.trim().to_string())
        .filter(|dir| !dir.is_empty());
    let previous_compact = config.compact_window.clone();
    next.compact_window = compact_window;
    next.language = pick(input.language, LANGUAGES);
    next.theme = pick(input.theme, THEMES);
    next.auto_update = input.auto_update;
    next.code_portable = input.code_portable;
    // Роль без провайдера или модели — не выбрана (пустой выбор в окне).
    let keep = |role: Option<crate::model::ModelRef>| {
        role.filter(|r| !r.provider.trim().is_empty() && !r.model.trim().is_empty())
    };
    next.model_roles = crate::model::ModelRoles {
        sonnet: keep(input.model_roles.sonnet),
        critic: keep(input.model_roles.critic),
    };
    // Основная модель — модель шлюза по умолчанию (store::build_file): нужен перезапуск.
    // Критик шлюзу не нужен (ADR-012).
    let port_changed = port_changed || next.model_roles.sonnet != config.model_roles.sonnet;
    store::save(&shared.config_dir, &next).map_err(|exc| exc.to_string())?;
    *config = next;
    crate::compact::sync(&config, previous_compact.as_deref());
    crate::i18n::apply(config.language.as_deref());
    crate::tray::relabel(&app);
    let ui = store::ui_config(&config, &shared.config_dir);
    drop(config);
    if port_changed {
        runtime::stop(&shared);
    }
    Ok(ui)
}

const LANGUAGES: &[&str] = &["ru", "en"];
const THEMES: &[&str] = &["light", "dark"];

/// Выбор из допустимых значений; «auto», пусто и чужое — `None` («как в Windows»).
fn pick(value: Option<String>, allowed: &[&str]) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| allowed.contains(&value.as_str()))
}

/// Быстрые переключатели в заголовке окна: тема и язык, без остальных настроек.
/// `theme`: `light` / `dark` / `auto`; `language`: `ru` / `en` / `auto`; `None` — не менять.
#[tauri::command(async)]
pub fn set_ui(
    app: tauri::AppHandle,
    shared: State<'_, AppShared>,
    theme: Option<String>,
    language: Option<String>,
) -> Result<UiConfig, String> {
    let mut config = shared.config.lock().unwrap();
    let mut next = config.clone();
    if theme.is_some() {
        next.theme = pick(theme, THEMES);
    }
    if language.is_some() {
        next.language = pick(language, LANGUAGES);
    }
    store::save(&shared.config_dir, &next).map_err(|exc| exc.to_string())?;
    *config = next;
    crate::i18n::apply(config.language.as_deref());
    crate::tray::relabel(&app);
    Ok(store::ui_config(&config, &shared.config_dir))
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

/// Открыт ли Claude в профиле PGC и применены ли ему нынешние настройки (строка статуса).
#[tauri::command]
pub fn claude_state(shared: State<'_, AppShared>) -> crate::claude::ClaudeRun {
    let config = shared.config.lock().unwrap().clone();
    crate::claude::run_state(&config, &runtime::status(&shared))
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
) -> Result<crate::model::LaunchResult, String> {
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
    let limits = main_model_limits(&config, &status).await;
    // Обход PATH (сетевые диски) — не в главном потоке окна.
    tauri::async_runtime::spawn_blocking(move || {
        // `/model` в консоли пишет в общие настройки: что было до неё — вернуть при выходе.
        crate::codemodel::remember(&config);
        claude::launch_code(&config, &status, &limits)
    })
    .await
    .map_err(|exc| exc.to_string())?
}

/// «Подключить проект», шаг 1: что мешает и что нужно знать. Ничего не меняет (ADR-014).
#[tauri::command]
pub async fn connect_check(
    shared: State<'_, AppShared>,
    dir: String,
) -> Result<crate::connect::Check, String> {
    let config = shared.config.lock().unwrap().clone();
    let status = runtime::status(&shared);
    tauri::async_runtime::spawn_blocking(move || {
        let dir = std::path::PathBuf::from(dir.trim());
        if !dir.is_dir() {
            let shown = dir.display();
            return Err(tr!(
                "Папка не найдена: {shown}",
                "Folder not found: {shown}"
            ));
        }
        Ok(crate::connect::decide(&crate::connect::facts(
            &config, &status, &dir,
        )))
    })
    .await
    .map_err(|exc| exc.to_string())?
}

/// Осмотр и план подключённого проекта.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectState {
    pub root: String,
    pub inspect: serde_json::Value,
    /// План `setup`; `None` — установленная копия плана ещё не отдаёт (`plan_error`).
    pub plan: Option<Vec<crate::connect::PlanItem>>,
    pub plan_error: Option<String>,
    /// Проверки проекта с галочками (ADR-016); `None` — копия agent-kit их ещё не отдаёт.
    pub checks: Option<crate::connect::Proposal>,
    /// Плагин только что включён: открытые сессии Claude его не увидят (случай 18).
    pub fresh: bool,
    /// Ответ `setup --apply`: что применено и что пропущено.
    pub applied: Option<serde_json::Value>,
}

fn connect_state(kit: &std::path::Path, root: &std::path::Path) -> Result<ConnectState, String> {
    let inspect = crate::connect::inspect(kit, root)?;
    let (plan, plan_error) = match crate::connect::plan(kit, root) {
        Ok(plan) => (Some(plan), None),
        Err(exc) => (None, Some(exc)),
    };
    Ok(ConnectState {
        root: root.display().to_string(),
        inspect,
        plan,
        plan_error,
        // Старая копия без `--propose` — просто без списка проверок, как раньше.
        checks: crate::connect::propose(kit, root).ok(),
        fresh: false,
        applied: None,
    })
}

/// Шаг 2: включить agent-kit (если ещё не включён, с обновлением копии), осмотр и план.
/// Повторная проверка предусловий: окно могло отстать от того, что на диске.
#[tauri::command]
pub async fn connect_open(
    shared: State<'_, AppShared>,
    dir: String,
    team: bool,
) -> Result<ConnectState, String> {
    let config = shared.config.lock().unwrap().clone();
    let status = runtime::status(&shared);
    tauri::async_runtime::spawn_blocking(move || {
        let facts = crate::connect::facts(&config, &status, std::path::Path::new(dir.trim()));
        let check = crate::connect::decide(&facts);
        if let Some(first) = check.blockers.first() {
            return Err(first.text.clone());
        }
        let (kit, fresh) = crate::connect::open(&config, &facts, team)?;
        let root = facts.root.clone().unwrap_or_default();
        Ok(ConnectState {
            fresh,
            ..connect_state(&kit, &root)?
        })
    })
    .await
    .map_err(|exc| exc.to_string())?
}

/// План отключения проекта (ADR-017): что убрать из файлов и как выключить плагин.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UndoState {
    pub root: String,
    pub items: Vec<crate::connect::PlanItem>,
    /// Копия agent-kit не умеет откат: файлы — руками (случай 37).
    pub kit_old: bool,
    /// Ответ последнего «Отключить отмеченное».
    pub applied: Option<serde_json::Value>,
}

fn undo_state(config: &Config, facts: &crate::connect::Facts) -> Result<UndoState, String> {
    let root = facts.root.clone().ok_or_else(|| {
        tr!(
            "папка — не git-репозиторий",
            "the folder is not a git repository"
        )
    })?;
    let kit = crate::connect::installed(config, &root)?;
    let kit_items = crate::connect::undo_plan(&kit, &root);
    let kit_old = kit_items.is_none();
    let mut items = kit_items.unwrap_or_default();
    items.extend(crate::connect::plugin_undo_items(
        facts.installed_scope.as_deref(),
    ));
    Ok(UndoState {
        root: root.display().to_string(),
        items,
        kit_old,
        applied: None,
    })
}

/// «Отключить…»: показать план отключения, ничего не меняя.
#[tauri::command]
pub async fn connect_undo_open(
    shared: State<'_, AppShared>,
    dir: String,
) -> Result<UndoState, String> {
    let config = shared.config.lock().unwrap().clone();
    let status = runtime::status(&shared);
    tauri::async_runtime::spawn_blocking(move || {
        let facts = crate::connect::facts(&config, &status, std::path::Path::new(dir.trim()));
        undo_state(&config, &facts)
    })
    .await
    .map_err(|exc| exc.to_string())?
}

/// «Отключить отмеченное»: только отмеченные пункты; план строится заново на этом шаге.
#[tauri::command]
pub async fn connect_undo_apply(
    shared: State<'_, AppShared>,
    dir: String,
    chosen: Vec<String>,
) -> Result<UndoState, String> {
    let config = shared.config.lock().unwrap().clone();
    let status = runtime::status(&shared);
    tauri::async_runtime::spawn_blocking(move || {
        let facts = crate::connect::facts(&config, &status, std::path::Path::new(dir.trim()));
        let root = facts.root.clone().ok_or_else(|| {
            tr!(
                "папка — не git-репозиторий",
                "the folder is not a git repository"
            )
        })?;
        let kit = crate::connect::installed(&config, &root)?;
        let applied = crate::connect::undo_apply(
            &config,
            &kit,
            &root,
            &chosen,
            facts.installed_scope.as_deref(),
            facts.user_install,
        )?;
        // After the plugin is off there is no installed copy to ask: show what was done.
        let state = undo_state(&config, &facts).unwrap_or(UndoState {
            root: root.display().to_string(),
            items: Vec::new(),
            kit_old: false,
            applied: None,
        });
        Ok(UndoState {
            applied: Some(applied),
            ..state
        })
    })
    .await
    .map_err(|exc| exc.to_string())?
}

/// Обновить agent-kit проекта до встроенного в PGC — по кнопке (случай 24).
#[tauri::command]
pub async fn connect_update(
    shared: State<'_, AppShared>,
    dir: String,
) -> Result<ConnectState, String> {
    let config = shared.config.lock().unwrap().clone();
    let status = runtime::status(&shared);
    tauri::async_runtime::spawn_blocking(move || {
        let facts = crate::connect::facts(&config, &status, std::path::Path::new(dir.trim()));
        let (kit, fresh) = crate::connect::update(&config, &facts)?;
        let root = facts.root.clone().unwrap_or_default();
        Ok(ConnectState {
            fresh,
            ..connect_state(&kit, &root)?
        })
    })
    .await
    .map_err(|exc| exc.to_string())?
}

/// Откуда брать agent-kit, если его маркетплейс не добавлен (блокер «источник», случай 9).
#[tauri::command]
pub async fn connect_set_source(
    shared: State<'_, AppShared>,
    source: String,
) -> Result<(), String> {
    let source = source.trim().to_string();
    let mut config = shared.config.lock().unwrap();
    let mut next = config.clone();
    next.kit_source = (!source.is_empty()).then_some(source);
    store::save(&shared.config_dir, &next).map_err(|exc| exc.to_string())?;
    *config = next;
    Ok(())
}

/// Шаг 3: применить выбранные пункты плана и показать новое состояние.
#[tauri::command]
pub async fn connect_apply(
    shared: State<'_, AppShared>,
    root: String,
    chosen: Vec<String>,
    checks: Vec<String>,
) -> Result<ConnectState, String> {
    let config = shared.config.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let root = std::path::PathBuf::from(root);
        // Корень пришёл из окна: заново убедиться, что это корень репозитория.
        let real = crate::connect::repo_root(&root);
        if real
            .as_deref()
            .is_none_or(|real| !crate::connect::same_path(real, &root))
        {
            let shown = root.display();
            return Err(tr!(
                "{shown} — не корень git-репозитория",
                "{shown} is not the root of a git repository"
            ));
        }
        let kit = crate::connect::installed(&config, &root)?;
        let applied = crate::connect::apply(&kit, &root, &chosen, &checks)?;
        Ok(ConnectState {
            applied: Some(applied),
            ..connect_state(&kit, &root)?
        })
    })
    .await
    .map_err(|exc| exc.to_string())?
}

/// Окно и предел вывода основной модели из каталога OpenCode. Каталог не загрузился за
/// CATALOG_GRACE или модели в нём нет — пусто, Claude Code обойдётся прежними значениями.
async fn main_model_limits(config: &Config, status: &UiStatus) -> crate::catalog::Hint {
    let Some(main) = claude::main_model(status, &claude::role_aliases(config)) else {
        return Default::default();
    };
    let Some(provider) = config.providers.iter().find(|p| p.id == main.provider_id) else {
        return Default::default();
    };
    let Ok(Some(catalog)) = tokio::time::timeout(CATALOG_GRACE, crate::catalog::load()).await
    else {
        return Default::default();
    };
    let id = ctp_core::alias::strip_1m(&main.model).trim().to_string();
    crate::catalog::hints(&catalog, &provider.base_url, std::slice::from_ref(&id))
        .models
        .remove(&id)
        .unwrap_or_default()
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

/// «Проверить обновления»: результат запоминается для статуса окна.
#[tauri::command]
pub async fn check_update(
    app: tauri::AppHandle,
    shared: State<'_, AppShared>,
) -> Result<Option<crate::updates::UpdateInfo>, String> {
    let found = crate::updates::check(&app).await?;
    *shared.update.lock().unwrap() = found.clone();
    Ok(found)
}

/// Скачивает, проверяет подпись, ставит и перезапускает программу.
#[tauri::command]
pub async fn install_update(app: tauri::AppHandle) -> Result<(), String> {
    crate::updates::install(&app).await
}

/// Итог скачивания Claude: что сказать и обновлённые настройки окна.
#[derive(serde::Serialize)]
pub struct Downloaded {
    message: String,
    config: UiConfig,
}

/// Прогресс загрузки в окно (событие `download`), не чаще пяти раз в секунду.
fn progress_to(
    app: &tauri::AppHandle,
    id: &'static str,
) -> impl Fn(u64, Option<u64>) + Send + Sync + use<> {
    use tauri::Emitter;
    let app = app.clone();
    let last = std::sync::Mutex::new(std::time::Instant::now() - std::time::Duration::from_secs(1));
    move |done, total| {
        let mut last = last.lock().unwrap_or_else(|e| e.into_inner());
        if last.elapsed() >= std::time::Duration::from_millis(200) || Some(done) == total {
            *last = std::time::Instant::now();
            let _ = app.emit(
                "download",
                serde_json::json!({"id": id, "done": done, "total": total}),
            );
        }
    }
}

/// Одна загрузка каждого вида за раз: повторный клик не запускает вторую.
static DOWNLOADING: std::sync::Mutex<Vec<&'static str>> = std::sync::Mutex::new(Vec::new());

struct Busy(&'static str);

impl Busy {
    fn take(id: &'static str) -> Result<Self, String> {
        let mut list = DOWNLOADING.lock().unwrap_or_else(|e| e.into_inner());
        if list.contains(&id) {
            return Err(tr!("Уже скачивается", "Already downloading"));
        }
        list.push(id);
        Ok(Self(id))
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        DOWNLOADING
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|id| *id != self.0);
    }
}

/// «Скачать Claude Code»: портативно (в папку приложения) или обычной установкой. Кнопка
/// «Claude Code» дальше открывает выбранный вариант.
#[tauri::command]
pub async fn download_code(
    app: tauri::AppHandle,
    shared: State<'_, AppShared>,
    portable: bool,
) -> Result<Downloaded, String> {
    let _busy = Busy::take("code")?;
    let progress = progress_to(&app, "code");
    let message = crate::installs::get_code(portable, &progress)
        .await
        .map_err(|exc| format!("{exc:#}"))?;
    let mut config = shared.config.lock().unwrap();
    let mut next = config.clone();
    next.code_portable = portable;
    store::save(&shared.config_dir, &next).map_err(|exc| exc.to_string())?;
    *config = next;
    Ok(Downloaded {
        message,
        config: store::ui_config(&config, &shared.config_dir),
    })
}

/// «Скачать Claude Desktop»: портативно — распаковка в папку приложения, и «Открыть
/// Claude» запускает её; иначе — официальный установщик (окно спрашивает заранее, если
/// Claude уже установлен: это обычный Claude человека).
#[tauri::command]
pub async fn download_desktop(
    app: tauri::AppHandle,
    shared: State<'_, AppShared>,
    portable: bool,
) -> Result<Downloaded, String> {
    let _busy = Busy::take("desktop")?;
    let progress = progress_to(&app, "desktop");
    let result = crate::installs::get_desktop(portable, &progress)
        .await
        .map_err(|exc| format!("{exc:#}"))?;
    let mut config = shared.config.lock().unwrap();
    let mut next = config.clone();
    if let Some(exe) = &result.exe {
        next.claude_path = Some(exe.display().to_string());
    } else if next
        .claude_path
        .as_deref()
        .is_some_and(|path| crate::installs::is_portable_desktop(std::path::Path::new(path)))
    {
        // Поставили обычный Claude — «Открыть Claude» берёт его, а не портативный.
        next.claude_path = None;
    }
    store::save(&shared.config_dir, &next).map_err(|exc| exc.to_string())?;
    *config = next;
    let ui = store::ui_config(&config, &shared.config_dir);
    let check = config.clone();
    drop(config);
    // Новый Claude — новые правила моделей и самопроверка.
    let dir = shared.config_dir.clone();
    std::thread::spawn(move || crate::upkeep::check_into(&app, &check, &dir, true));
    Ok(Downloaded {
        message: result.message,
        config: ui,
    })
}

// ---------- прокси (ядро mihomo) ----------

use crate::mihomo::{self, ProxyState};

/// Сохраняет настройки, изменённые `change`, сначала на диск, потом в память.
fn update_config(
    shared: &AppShared,
    change: impl FnOnce(&mut crate::model::Config),
) -> Result<crate::model::Config, String> {
    let mut config = shared.config.lock().unwrap();
    let mut next = config.clone();
    change(&mut next);
    store::save(&shared.config_dir, &next).map_err(|exc| exc.to_string())?;
    *config = next.clone();
    Ok(next)
}

/// Режим целиком (ядро, системный прокси, прокси шлюза); сменившийся порт — в настройки;
/// работающий шлюз перезапускается, чтобы его клиенты взяли новый прокси.
/// Настройки читаются под блокировкой ядра (`mihomo::apply_with`): из быстрых смен режима
/// применяется последняя сохранённая.
fn current_config(app: &tauri::AppHandle) -> crate::model::Config {
    use tauri::Manager;
    app.state::<AppShared>().config.lock().unwrap().clone()
}

async fn apply_proxy(app: &tauri::AppHandle, shared: &AppShared) -> Result<(), String> {
    let handle = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        mihomo::apply_with(|| current_config(&handle))
    })
    .await
    .map_err(|exc| exc.to_string())?;
    if let Ok(Some(port)) = result {
        update_config(shared, |c| c.proxy.port = port)?;
    }
    if runtime::status(shared).running {
        runtime::start(shared).await?;
    }
    result.map(|_| ()).map_err(|exc| format!("{exc:#}"))
}

/// Старт приложения с включённым прокси (поток из `setup`). Пришлось сменить порт — шлюз,
/// уже поднятый окном со старым адресом, перезапускается.
pub fn start_proxy_in_background(app: &tauri::AppHandle) {
    use tauri::Manager;
    let shared = app.state::<AppShared>();
    match mihomo::apply_with(|| current_config(app)) {
        Ok(Some(port)) => {
            let _ = update_config(&shared, |c| c.proxy.port = port);
            if runtime::status(&shared).running
                && let Err(exc) = tauri::async_runtime::block_on(runtime::start(&shared))
            {
                tracing::warn!(error = %exc, "gateway not restarted on the new proxy port");
            }
        }
        Ok(None) => {}
        Err(exc) => tracing::warn!(error = %format!("{exc:#}"), "proxy not started"),
    }
}

async fn proxy_state_of(shared: &AppShared) -> ProxyState {
    let config = shared.config.lock().unwrap().clone();
    mihomo::state(&config).await
}

#[tauri::command]
pub async fn proxy_state(shared: State<'_, AppShared>) -> Result<ProxyState, String> {
    Ok(proxy_state_of(&shared).await)
}

/// Режим прокси: `off`, `app`, `system`, `tun`. Порт выбирается сразу, чтобы при сбое ядра
/// шлюз не уходил напрямую.
#[tauri::command]
pub async fn proxy_set_mode(
    app: tauri::AppHandle,
    shared: State<'_, AppShared>,
    mode: String,
) -> Result<ProxyState, String> {
    if !crate::model::PROXY_MODES.contains(&mode.as_str()) {
        return Err(format!("unknown proxy mode {mode}"));
    }
    let (previous, port) = {
        let config = shared.config.lock().unwrap();
        (config.proxy.mode.clone(), config.proxy.port)
    };
    let port = match port {
        0 => mihomo::pick_port().map_err(|exc| exc.to_string())?,
        port => port,
    };
    let tun = mode == "tun";
    update_config(&shared, |c| {
        c.proxy.mode = mode;
        c.proxy.port = port;
    })?;
    if let Err(exc) = apply_proxy(&app, &shared).await {
        // TUN не поднялся (отказ в UAC и т. п.): прежний режим, иначе каждый запуск PGC
        // снова спрашивал бы права, а шлюз всё это время не ходил бы никуда.
        if tun {
            update_config(&shared, |c| c.proxy.mode = previous)?;
            let _ = apply_proxy(&app, &shared).await;
        }
        return Err(exc);
    }
    Ok(proxy_state_of(&shared).await)
}

/// Скачать ядро; если режим включён — сразу запустить.
#[tauri::command]
pub async fn proxy_download_core(
    app: tauri::AppHandle,
    shared: State<'_, AppShared>,
) -> Result<ProxyState, String> {
    let _busy = Busy::take("core")?;
    // Работающее ядро держит свой exe: остановить, заменить, запустить снова.
    mihomo::stop();
    let progress = progress_to(&app, "core");
    let downloaded = mihomo::download_core(&progress).await;
    let on = shared.config.lock().unwrap().proxy.is_on();
    if downloaded.is_err() && on {
        // Скачать не вышло: вернуть прежнее ядро, если оно осталось, но ответить именно
        // про скачивание — иначе человек чинит не то.
        let _ = apply_proxy(&app, &shared).await;
    }
    downloaded.map_err(|exc| format!("{exc:#}"))?;
    if on {
        apply_proxy(&app, &shared).await?;
    }
    Ok(proxy_state_of(&shared).await)
}

/// Работающему ядру — новый список подписок (без перезапуска). Ошибка не отменяет уже
/// сохранённую подписку: её покажет состояние, а повторное «Добавить» не создаст дубль.
async fn reload_proxy(app: &tauri::AppHandle) {
    let handle = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        mihomo::reload_with(|| current_config(&handle))
    })
    .await;
    if let Ok(Err(exc)) = result {
        tracing::warn!(error = %format!("{exc:#}"), "core config not reloaded");
    }
}

#[tauri::command]
pub async fn proxy_add_sub(
    app: tauri::AppHandle,
    shared: State<'_, AppShared>,
    name: String,
    url: String,
    hours: u32,
) -> Result<ProxyState, String> {
    let url = url.trim().to_string();
    let parsed = reqwest::Url::parse(&url).map_err(|_| {
        tr!(
            "Адрес подписки должен начинаться с https:// или http://",
            "The subscription address must start with https:// or http://"
        )
    })?;
    if !matches!(parsed.scheme(), "https" | "http") || parsed.host_str().is_none() {
        return Err(tr!(
            "Адрес подписки должен начинаться с https:// или http://",
            "The subscription address must start with https:// or http://"
        ));
    }
    let name = match name.trim() {
        "" => mihomo::host_of(&url),
        name => name.to_string(),
    };
    let id = format!("s{}", &uuid::Uuid::new_v4().simple().to_string()[..6]);
    store::set_key(&mihomo::sub_key(&id), &url)?;
    let sub = crate::model::Subscription {
        id: id.clone(),
        name,
        interval_hours: hours.clamp(1, 168),
    };
    if let Err(exc) = update_config(&shared, |c| c.proxy.subscriptions.push(sub)) {
        store::delete_key(&mihomo::sub_key(&id));
        return Err(exc);
    }
    reload_proxy(&app).await;
    Ok(proxy_state_of(&shared).await)
}

#[tauri::command]
pub async fn proxy_remove_sub(
    app: tauri::AppHandle,
    shared: State<'_, AppShared>,
    id: String,
) -> Result<ProxyState, String> {
    update_config(&shared, |c| c.proxy.subscriptions.retain(|s| s.id != id))?;
    store::delete_key(&mihomo::sub_key(&id));
    reload_proxy(&app).await;
    mihomo::forget_provider(&id);
    Ok(proxy_state_of(&shared).await)
}

#[tauri::command]
pub async fn proxy_update_sub(
    shared: State<'_, AppShared>,
    id: String,
) -> Result<ProxyState, String> {
    mihomo::update(&id)
        .await
        .map_err(|exc| format!("{exc:#}"))?;
    Ok(proxy_state_of(&shared).await)
}

#[tauri::command]
pub async fn proxy_select(
    shared: State<'_, AppShared>,
    name: String,
) -> Result<ProxyState, String> {
    mihomo::select(&name)
        .await
        .map_err(|exc| format!("{exc:#}"))?;
    Ok(proxy_state_of(&shared).await)
}

#[tauri::command]
pub async fn proxy_measure(shared: State<'_, AppShared>) -> Result<ProxyState, String> {
    mihomo::measure().await.map_err(|exc| format!("{exc:#}"))?;
    Ok(proxy_state_of(&shared).await)
}

/// Полный выход (как «Выход» в трее): шлюз останавливается вместе с программой.
#[tauri::command]
pub fn app_exit(app: tauri::AppHandle) {
    mihomo::shutdown();
    crate::claude::repair_registry_on_exit(&app);
    crate::codemodel::restore_on_exit(&app);
    app.exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ProviderRow;
    use crate::store::fake_keys;

    /// Состояние приложения в своей папке, как после первого старта; Credential Manager —
    /// подмена этого потока (живёт, пока жив возвращаемый `Guard`).
    fn app() -> (AppShared, std::path::PathBuf, fake_keys::Guard) {
        let guard = fake_keys::install();
        let dir = std::env::temp_dir()
            .join("agents")
            .join(format!("ctp-cmd-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let config = store::load_or_create(&dir).unwrap();
        (AppShared::new(dir.clone(), config), dir, guard)
    }

    fn provider(id: &str) -> ProviderRow {
        ProviderRow {
            id: id.into(),
            kind: "openai".into(),
            base_url: "https://api.example.com/v1".into(),
            models: vec!["m1".into()],
            ..Default::default()
        }
    }

    fn input(
        id: &str,
        original: Option<&str>,
        key: Option<&str>,
        clear: bool,
    ) -> SaveProviderInput {
        SaveProviderInput {
            provider: provider(id),
            original_id: original.map(str::to_string),
            api_key: key.map(str::to_string),
            clear_key: clear,
        }
    }

    fn key_of(id: &str) -> Option<String> {
        fake_keys::with(|fake| fake.keys.get(id).cloned()).flatten()
    }

    fn ids(shared: &AppShared) -> Vec<String> {
        let config = shared.config.lock().unwrap();
        config.providers.iter().map(|p| p.id.clone()).collect()
    }

    /// Папка на месте временного файла: запись конфига не удаётся (а прежний остаётся целым).
    fn block_config_writes(dir: &std::path::Path) {
        std::fs::create_dir(dir.join("config.json.ctp-tmp")).unwrap();
    }

    #[test]
    fn a_new_provider_gets_its_key_and_a_row_in_the_config() {
        let (shared, dir, _fake) = app();
        save_provider_in(&shared, input("p1", None, Some(" sk-1 "), false)).unwrap();
        assert_eq!(key_of("p1").as_deref(), Some("sk-1"));
        assert_eq!(ids(&shared), ["p1"]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_rename_carries_the_key_over_and_drops_the_old_one() {
        let (shared, dir, _fake) = app();
        save_provider_in(&shared, input("p1", None, Some("sk-1"), false)).unwrap();
        save_provider_in(&shared, input("p2", Some("p1"), None, false)).unwrap();
        assert_eq!(key_of("p2").as_deref(), Some("sk-1"));
        assert_eq!(key_of("p1"), None);
        assert_eq!(ids(&shared), ["p2"]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_failed_config_write_gives_the_previous_key_back() {
        let (shared, dir, _fake) = app();
        save_provider_in(&shared, input("p1", None, Some("old"), false)).unwrap();
        block_config_writes(&dir);
        // A new key for an existing provider: the old one must come back.
        assert!(save_provider_in(&shared, input("p1", Some("p1"), Some("new"), false)).is_err());
        assert_eq!(key_of("p1").as_deref(), Some("old"));
        // A key for a provider that did not exist: nothing is left behind.
        assert!(save_provider_in(&shared, input("p9", None, Some("sk-9"), false)).is_err());
        assert_eq!(key_of("p9"), None);
        assert_eq!(ids(&shared), ["p1"]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_failed_write_after_a_rename_with_a_new_key_leaves_both_ids_as_they_were() {
        let (shared, dir, _fake) = app();
        save_provider_in(&shared, input("p1", None, Some("old"), false)).unwrap();
        block_config_writes(&dir);
        assert!(save_provider_in(&shared, input("p2", Some("p1"), Some("new"), false)).is_err());
        assert_eq!(key_of("p1").as_deref(), Some("old"));
        assert_eq!(key_of("p2"), None);
        assert_eq!(ids(&shared), ["p1"]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn clearing_the_key_deletes_it_and_an_orphan_under_a_new_id_is_not_inherited() {
        let (shared, dir, _fake) = app();
        save_provider_in(&shared, input("p1", None, Some("sk-1"), false)).unwrap();
        save_provider_in(&shared, input("p1", Some("p1"), None, true)).unwrap();
        assert_eq!(key_of("p1"), None);
        // A key left under an id by an earlier failure must not go to a new provider with it.
        fake_keys::with(|fake| fake.keys.insert("p3".into(), "orphan".into()));
        save_provider_in(&shared, input("p3", None, None, false)).unwrap();
        assert_eq!(key_of("p3"), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_key_that_cannot_be_read_stops_a_rename_instead_of_losing_it() {
        let (shared, dir, _fake) = app();
        save_provider_in(&shared, input("p1", None, Some("sk-1"), false)).unwrap();
        fake_keys::with(|fake| fake.fail_read = true);
        assert!(save_provider_in(&shared, input("p2", Some("p1"), None, false)).is_err());
        fake_keys::with(|fake| fake.fail_read = false);
        assert_eq!(key_of("p1").as_deref(), Some("sk-1"));
        assert_eq!(ids(&shared), ["p1"]);
        // The key store refuses a write: the config stays as it was.
        fake_keys::with(|fake| fake.fail_write = true);
        assert!(save_provider_in(&shared, input("p5", None, Some("sk-5"), false)).is_err());
        assert_eq!(ids(&shared), ["p1"]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn deleting_a_provider_keeps_its_key_when_the_config_cannot_be_written() {
        let (shared, dir, _fake) = app();
        save_provider_in(&shared, input("p1", None, Some("sk-1"), false)).unwrap();
        block_config_writes(&dir);
        assert!(delete_provider_in(&shared, "p1").is_err());
        assert_eq!(key_of("p1").as_deref(), Some("sk-1"));
        assert_eq!(ids(&shared), ["p1"]);
        std::fs::remove_dir(dir.join("config.json.ctp-tmp")).unwrap();
        delete_provider_in(&shared, "p1").unwrap();
        assert_eq!(key_of("p1"), None);
        assert!(ids(&shared).is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn theme_and_language_choices_keep_only_known_values() {
        let pick = |value: &str, allowed| pick(Some(value.to_string()), allowed);
        assert_eq!(pick("dark", THEMES).as_deref(), Some("dark"));
        assert_eq!(pick(" light ", THEMES).as_deref(), Some("light"));
        // «auto», пусто и чужое — как в Windows.
        assert_eq!(pick("auto", THEMES), None);
        assert_eq!(pick("", LANGUAGES), None);
        assert_eq!(pick("neon", THEMES), None);
        // Язык и тема не путаются между собой.
        assert_eq!(pick("ru", LANGUAGES).as_deref(), Some("ru"));
        assert_eq!(pick("ru", THEMES), None);
        assert_eq!(super::pick(None, THEMES), None);
    }
}
