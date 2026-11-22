use crate::model::{Config, UiStatus};
use crate::tr;
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Постоянный ID записи в `configLibrary`: Claude видит ту же конфигурацию от запуска к запуску.
const LIBRARY_ID: &str = "6b1f3c52-7a0e-4f7e-9d4b-c1a0de9a7e3d";
/// Без неё Claude Code объявляет советника (`/advisor`) только прямому API Anthropic.
const ADVISOR_ENV: &str = "CLAUDE_CODE_ENABLE_EXPERIMENTAL_ADVISOR_TOOL";

pub fn detect(config: &Config) -> Option<PathBuf> {
    if let Some(path) = config
        .claude_path
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        let path = PathBuf::from(path);
        let path = if path.is_dir() {
            path.join("claude.exe")
        } else {
            path
        };
        if path.is_file() {
            return Some(path);
        }
    }

    let local = local_appdata()?;
    let base = PathBuf::from(&local).join("AnthropicClaude");
    let mut versions: Vec<(Vec<u64>, PathBuf)> = Vec::new();
    if let Ok(entries) = fs::read_dir(&base) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with("app-") {
                continue;
            }
            let exe = entry.path().join("claude.exe");
            if !exe.is_file() {
                continue;
            }
            versions.push((version_key(&name), exe));
        }
    }
    versions.sort_by(|left, right| left.0.cmp(&right.0));
    if let Some((_, exe)) = versions.pop() {
        return Some(exe);
    }

    let stub = base.join("claude.exe");
    if stub.is_file() {
        return Some(stub);
    }
    let programs = local.join("Programs").join("claude").join("claude.exe");
    programs.is_file().then_some(programs)
}

fn version_key(name: &str) -> Vec<u64> {
    name.trim_start_matches("app-")
        .split('.')
        .filter_map(|part| part.parse().ok())
        .collect()
}

/// Русская копия установленной версии; любая ошибка — повод запустить оригинал.
fn russian_copy(exe: &Path, copies: &Path, config_dir: &Path) -> anyhow::Result<PathBuf> {
    use anyhow::Context;
    let file = config_dir.join(crate::ruui::TRANSLATION_FILE);
    let ru = fs::read(&file).with_context(|| {
        let file = file.display();
        tr!("нет файла перевода {file}", "no translation file {file}")
    })?;
    let app_dir = exe
        .parent()
        .with_context(|| tr!("у claude.exe нет папки", "claude.exe has no folder"))?;
    let dir = app_dir.display();
    anyhow::ensure!(
        app_dir.join("resources").join("ion-dist.asar").is_file(),
        "{}",
        tr!(
            "{dir} — не папка версии Claude (нет resources\\ion-dist.asar)",
            "{dir} is not a Claude version folder (no resources\\ion-dist.asar)"
        )
    );
    crate::ruui::ensure_copy(app_dir, copies, &ru)
}

/// Переменные, которые оставляет Claude Code / Desktop, если приложение запущено из
/// него: дочерний `claude` счёл бы себя частью чужой сессии или ушёл бы мимо шлюза.
const HOST_ENV_PREFIXES: [&str; 22] = [
    "ANTHROPIC_",
    // Bedrock / Vertex / Foundry увели бы CLI мимо шлюза.
    "CLAUDE_CODE_USE_",
    "CLAUDECODE",
    "AI_AGENT",
    "CLAUDE_CODE_SIMPLE",
    "CLAUDE_EFFORT",
    "CLAUDE_CODE_TERMINAL",
    "CLAUDE_CODE_EMIT_",
    "CLAUDE_CODE_USER_EMAIL",
    "CLAUDE_CODE_SESSION",
    "CLAUDE_CODE_CHILD",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_HOST",
    "CLAUDE_CODE_MESSAGING",
    "CLAUDE_CODE_OAUTH",
    "CLAUDE_CODE_SDK",
    "CLAUDE_AGENT_SDK",
    "CLAUDE_PID",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_CODE_DESKTOP",
    "CLAUDE_CODE_ACCOUNT",
    "CLAUDE_CODE_ORGANIZATION",
];

/// Какие переменные убрать и какие задать.
pub type CodeEnv = (Vec<String>, Vec<(String, String)>);

/// Псевдонимы моделей, выбранных на роли `sonnet`/`opus`/`haiku` (`None` — не выбрана
/// или её больше нет у провайдера).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RoleAliases {
    pub sonnet: Option<String>,
    pub opus: Option<String>,
    pub haiku: Option<String>,
}

/// Советника шлюз исполняет, только если сильная модель (роль opus) выбрана, она ещё
/// есть у провайдера и провайдер OpenAI-совместимый. По конфигу: шлюз может быть
/// остановлен.
pub fn advisor_available(config: &Config) -> bool {
    let Some(strong) = config.model_roles.opus.as_ref() else {
        return false;
    };
    crate::store::role_alias(config, Some(strong)).is_some()
        && config
            .providers
            .iter()
            .any(|p| p.id == strong.provider && !p.kind.eq_ignore_ascii_case("anthropic"))
}

pub fn role_aliases(config: &Config) -> RoleAliases {
    let roles = &config.model_roles;
    RoleAliases {
        sonnet: crate::store::role_alias(config, roles.sonnet.as_ref()),
        opus: crate::store::role_alias(config, roles.opus.as_ref()),
        haiku: crate::store::role_alias(config, roles.haiku.as_ref()),
    }
}

/// Окружение Claude Code через шлюз: что убрать и что задать. Основная модель — роль
/// `sonnet` или первая модель шлюза. Алиасы `sonnet`/`opus` задаются, только если роль
/// выбрана: иначе Claude Code шлёт свой встроенный ID, и шлюз отдаёт его настоящей
/// модели Claude (если она есть у провайдера) или модели по умолчанию — основной
/// (`store::build_file`). Так же ведёт себя Desktop с невыбранной ролью.
pub fn code_env(
    status: &UiStatus,
    roles: &RoleAliases,
    inherited: impl Iterator<Item = String>,
) -> Result<CodeEnv, String> {
    let first = status.models.first().ok_or_else(|| {
        tr!(
            "В шлюзе нет моделей: добавьте провайдера и модель",
            "The gateway has no models: add a provider and a model"
        )
    })?;
    // Роль, чьей модели нет среди запущенных (шлюз не перезапускали), — как не выбранная.
    let pick = |alias: &Option<String>| {
        alias
            .as_ref()
            .and_then(|alias| status.models.iter().find(|m| &m.alias == alias))
    };
    let sonnet = pick(&roles.sonnet);
    let main = sonnet.unwrap_or(first);
    let opus = pick(&roles.opus);
    let haiku = pick(&roles.haiku).unwrap_or(main);
    // Имя шлюза Claude Code не знает и считает окно 1M — у модели без `[1m]` сжатие
    // разговора началось бы слишком поздно. Как в Claude Desktop: без `[1m]` — 200K.
    // По основной модели: на ней сессия и наследующие её субагенты (Explore, Plan).
    let window = if main.supports_1m {
        "1000000"
    } else {
        "200000"
    };
    let remove = inherited
        .filter(|name| {
            let upper = name.to_ascii_uppercase();
            HOST_ENV_PREFIXES.iter().any(|p| upper.starts_with(p))
        })
        .collect();
    let mut set: Vec<(String, String)> = vec![
        (
            "ANTHROPIC_BASE_URL".into(),
            format!("http://127.0.0.1:{}", status.port),
        ),
        ("ANTHROPIC_AUTH_TOKEN".into(), status.token.clone()),
        ("ANTHROPIC_MODEL".into(), main.alias.clone()),
    ];
    if let Some(sonnet) = sonnet {
        set.push((
            "ANTHROPIC_DEFAULT_SONNET_MODEL".into(),
            sonnet.alias.clone(),
        ));
    }
    if let Some(opus) = opus {
        set.push(("ANTHROPIC_DEFAULT_OPUS_MODEL".into(), opus.alias.clone()));
        // fable — уровень выше opus (советники, `model: fable`): та же сильная модель.
        set.push(("ANTHROPIC_DEFAULT_FABLE_MODEL".into(), opus.alias.clone()));
        // Советник (`/advisor`): Claude Code объявляет его шлюзу, шлюз ведёт к сильной —
        // если она на OpenAI-совместимом провайдере (другого советника шлюз не исполняет).
        if opus.openai {
            set.push((ADVISOR_ENV.into(), "1".into()));
        }
    }
    set.extend([
        // haiku задавалась и до ролей: фоновые мелочи — на основной, если быстрой нет.
        ("ANTHROPIC_DEFAULT_HAIKU_MODEL".into(), haiku.alias.clone()),
        // Устаревшее имя той же роли: его ещё читают старые версии Claude Code.
        ("ANTHROPIC_SMALL_FAST_MODEL".into(), haiku.alias.clone()),
        ("CLAUDE_CODE_MAX_CONTEXT_TOKENS".into(), window.into()),
    ]);
    Ok((remove, set))
}

/// Команда PowerShell для своего терминала («Скопировать команду») — те же переменные,
/// что задаёт кнопка «Claude Code» (чужие переменные своего терминала человек видит сам).
pub fn code_command(status: &UiStatus, roles: &RoleAliases) -> Result<String, String> {
    let (_, set) = code_env(status, roles, std::iter::empty())?;
    let mut lines: Vec<String> = set
        .iter()
        .map(|(name, value)| format!("$env:{name}=\"{value}\""))
        .collect();
    lines.push("claude".into());
    Ok(lines.join("\n"))
}

/// Переменная, через которую внутренний `cmd` получает путь к `claude`.
const CLI_VAR: &str = "CTP_CLAUDE_CLI";

/// Командная строка внешнего `cmd`. Путь к `claude` в неё не попадает: внутренний
/// `cmd /k` раскрывает `%CTP_CLAUDE_CLI%` один раз и в кавычках, поэтому ни `%`, ни
/// `&`, ни скобки в пути ничего не ломают. `%^…%` не даёт раскрыть переменную внешнему
/// `cmd` (переменной `^CTP_CLAUDE_CLI` нет, а `^` он потом снимает).
fn code_command_line() -> String {
    format!("/c start \"Claude Code\" cmd.exe /k \"\"%^{CLI_VAR}%\"\"")
}

/// `claude` из PATH (`claude.exe`, `claude.cmd`), иначе стандартная установка
/// `%USERPROFILE%\.local\bin\claude.exe`.
fn find_claude_cli() -> Option<PathBuf> {
    let from_path = std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path).find_map(|dir| {
            ["claude.exe", "claude.cmd"]
                .iter()
                .map(|name| dir.join(name))
                .find(|p| p.is_file())
        })
    });
    from_path.or_else(|| {
        let home = std::env::var_os("USERPROFILE")?;
        let exe = PathBuf::from(home)
            .join(".local")
            .join("bin")
            .join("claude.exe");
        exe.is_file().then_some(exe)
    })
}

/// Claude Code CLI в новом окне консоли через шлюз. Консоль создаётся сразу с нужным
/// окружением, поэтому его получает и Windows Terminal, если он терминал по умолчанию.
pub fn launch_code(config: &Config, status: &UiStatus) -> Result<String, String> {
    if !status.running {
        return Err(tr!("Сначала запустите шлюз", "Start the gateway first"));
    }
    // vars_os: значение не в Unicode не должно ронять запуск — нужны только имена.
    let (remove, set) = code_env(
        status,
        &role_aliases(config),
        std::env::vars_os().map(|(k, _)| k.to_string_lossy().into_owned()),
    )?;
    let cli = find_claude_cli().ok_or_else(|| {
        tr!(
            "Claude Code не найден: установите его (claude.ai/install) или добавьте claude в PATH",
            "Claude Code not found: install it (claude.ai/install) or add claude to PATH"
        )
    })?;
    let dir = config
        .code_dir
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
        .ok_or_else(|| {
            tr!(
                "Не задана папка для Claude Code",
                "No folder set for Claude Code"
            )
        })?;
    let dir = std::path::absolute(&dir).unwrap_or(dir);
    if !dir.is_dir() {
        let dir = dir.display();
        return Err(tr!(
            "Папка для Claude Code не найдена: {dir}",
            "Claude Code folder not found: {dir}"
        ));
    }
    // Через `start`: Rust всегда передаёт ребёнку свои std-потоки (у оконного приложения
    // их нет), и `cmd /k` с новой консолью, но пустым stdin сразу выходит. `start` даёт
    // окну собственную консоль; окружение и рабочая папка (`current_dir`) переходят как
    // есть — папка в командную строку не попадает, её имя командой не станет.
    // Внутренний `cmd /k` оставляет окно после выхода из claude — видно, чем он закончил.
    let mut command = Command::new("cmd.exe");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.raw_arg(code_command_line());
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: сам внешний cmd без окна
    }
    command.current_dir(&dir);
    for name in &remove {
        command.env_remove(name);
    }
    command.envs(set.iter().map(|(k, v)| (k, v)));
    command.env(CLI_VAR, &cli);
    command.spawn().map_err(|exc| {
        tr!(
            "Не удалось открыть консоль: {exc}",
            "Could not open a console: {exc}"
        )
    })?;
    let model = &set[2].1;
    let dir = dir.display();
    Ok(tr!(
        "Claude Code открыт в {dir} через шлюз (модель {model})",
        "Claude Code opened in {dir} through the gateway (model {model})"
    ))
}

/// Запись конфигурации Claude (3p): шлюз, ключ и модели. Отдельной функцией — её проверяет тест.
fn library_entry(
    config: &Config,
    status: &UiStatus,
    models: &[serde_json::Value],
) -> serde_json::Value {
    json!({
        "inferenceProvider": "gateway",
        "inferenceGatewayBaseUrl": format!("http://127.0.0.1:{}", status.port),
        "inferenceGatewayApiKey": status.token,
        "inferenceGatewayAuthScheme": "x-api-key",
        "inferenceCredentialKind": "static",
        "deploymentDisplayName": "Provider Gateway",
        "inferenceModels": models,
        // В 3p встроенный браузер выключен по умолчанию; галочка в настройках
        // включает его там же, где Claude читает остальные поля.
        "builtinBrowserEnabled": config.builtin_browser,
    })
}

pub fn launch(
    config: &Config,
    status: &UiStatus,
    config_dir: &Path,
) -> Result<crate::model::LaunchResult, String> {
    if !status.running {
        return Err(tr!("Сначала запустите шлюз", "Start the gateway first"));
    }
    if status.models.is_empty() {
        return Err(tr!(
            "В шлюзе нет моделей: добавьте провайдера и модель",
            "The gateway has no models: add a provider and a model"
        ));
    }
    let exe = detect(config).ok_or_else(|| {
        tr!(
            "Claude Desktop не найден. Укажите путь к claude.exe в настройках.",
            "Claude Desktop not found. Set the path to claude.exe in the settings."
        )
    })?;
    let base = base_dir().ok_or_else(|| {
        tr!(
            "Не найдена папка AppData\\Local",
            "AppData\\Local folder not found"
        )
    })?;
    let copies = base.join("claude-ru");
    let original = exe.clone();
    let (exe, russian, note) = if config.russian_ui {
        match russian_copy(&exe, &copies, config_dir) {
            Ok(copy) => (
                copy,
                true,
                tr!(" Русский интерфейс включён.", " Russian interface is on."),
            ),
            Err(exc) => {
                tracing::warn!(error = %format!("{exc:#}"), "Russian interface not applied");
                let note = tr!(
                    " Русский интерфейс не применён ({exc:#}) — запущен обычный Claude.",
                    " Russian interface not applied ({exc:#}); started the regular Claude."
                );
                (exe, false, note)
            }
        }
    } else {
        (exe, false, String::new())
    };
    let profile = base.join("Claude-3p");
    let library = profile.join("configLibrary");
    fs::create_dir_all(&library)
        .map_err(|exc| tr!("Профиль Claude: {exc}", "Claude profile: {exc}"))?;

    set_deployment_mode(&profile.join("claude_desktop_config.json"))?;

    // Профиль наш собственный: старые записи (в них локальный токен) убираем,
    // чтобы не копились по одной на каждый запуск.
    if let Ok(entries) = fs::read_dir(&library) {
        for entry in entries.flatten() {
            if entry.path().extension().is_some_and(|ext| ext == "json") {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
    let id = LIBRARY_ID;
    let models = inference_models(&status.models, &role_aliases(config));
    let entry = library_entry(config, status, &models);
    write_json(&library.join(format!("{id}.json")), &entry)?;
    write_json(
        &library.join("_meta.json"),
        &json!({"entries": [{"id": id, "name": "Provider Gateway"}], "appliedId": id}),
    )?;

    let mut command = Command::new(&exe);
    command
        .arg(format!("--user-data-dir={}", profile.display()))
        .env("LOCALAPPDATA", &base)
        .env("CLAUDE_USER_DATA_DIR", &profile)
        .current_dir(exe.parent().unwrap_or_else(|| Path::new(".")));
    // Настоящая AppData\Local для процессов, которые запускает копия (ruui::ENV_PRELUDE).
    // Из системной папки Windows, а не из окружения: приложение могли запустить из сессии
    // Claude, у которой оно само подменено.
    if let Some(real) = real_local_appdata() {
        command.env(crate::ruui::REAL_LOCALAPPDATA_VAR, real);
    }
    // Советник для сессий Code: Desktop передаёт им своё окружение. Как в консоли — только
    // если сильная модель на OpenAI-совместимом провайдере.
    if advisor_available(config) {
        command.env(ADVISOR_ENV, "1");
    }
    // Снимок до запуска: копия перепишет `claude://` и COM-класс на себя (проверено вживую).
    #[cfg(windows)]
    let before = russian.then(|| crate::ruui::registry::snapshot(&copies));
    #[cfg(not(windows))]
    let before: Option<Before> = None;
    let mut child = command.spawn().map_err(|exc| {
        let exe = exe.display();
        tr!(
            "Не удалось запустить {exe}: {exc}",
            "Could not start {exe}: {exc}"
        )
    })?;
    let pid = child.id();
    // Возвращаем ключи установленному Claude после старта копии и ещё раз, когда она
    // закроется. Если окно шлюза закроют раньше — это сделает следующий старт
    // (repair_registry, уже без снимка).
    if russian {
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(20));
            repair_for(&copies, &original, before.as_ref());
            let _ = child.wait();
            repair_for(&copies, &original, before.as_ref());
        });
    }
    let profile = profile.display();
    Ok(crate::model::LaunchResult {
        message: tr!(
            "Claude запущен (PID {pid}). Профиль: {profile}.{note}",
            "Claude started (PID {pid}). Profile: {profile}.{note}"
        ),
        // Русский просили, а открылся обычный Claude — окно покажет это как ошибку.
        warning: config.russian_ui && !russian,
    })
}

/// Настоящая папка AppData\Local (системная, SHGetKnownFolderPath), если она есть;
/// иначе `None`, и процессы копии останутся как раньше.
fn real_local_appdata() -> Option<PathBuf> {
    local_appdata().filter(|real| real.is_dir())
}

#[cfg(windows)]
type Before = crate::ruui::registry::Snapshot;
#[cfg(not(windows))]
type Before = ();

fn repair_for(copies: &Path, original_exe: &Path, before: Option<&Before>) {
    // Только для Claude в папке версии `…\app-X\claude.exe`: иначе «папка установки»
    // вычисляется неверно (заглушка Squirrel, Programs) и чинить не на что.
    #[cfg(windows)]
    if let Some(app) = original_exe.parent().filter(|app| {
        app.file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with("app-"))
    }) {
        let installed = app.parent().unwrap_or(app);
        crate::ruui::registry::repair(copies, installed, app, before);
    }
    #[cfg(not(windows))]
    let _ = (copies, original_exe, before);
}

/// При старте приложения: след русской копии в реестре, оставшийся с прошлого раза.
pub fn repair_registry(config: &Config) {
    let Some(base) = base_dir() else {
        return;
    };
    let copies = base.join("claude-ru");
    if let (true, Some(original)) = (copies.is_dir(), detect(config)) {
        repair_for(&copies, &original, None);
    }
}

/// Известная Claude Desktop модель с уровнями `low…max`: её поведение получает
/// модель OpenAI-провайдера, чтобы в выборе модели появились уровни рассуждения.
/// (Каталог моделей с `effort_levels` от шлюза Desktop не примет: он требует
/// подписи ключом Anthropic.)
pub const EFFORT_PROFILE: &str = "claude-opus-4-8";

/// Папка русских копий Claude (`%LOCALAPPDATA%\ClaudeProviderGateway\claude-ru`).
pub fn copies_root() -> anyhow::Result<PathBuf> {
    let base = base_dir().ok_or_else(|| {
        anyhow::anyhow!(
            "{}",
            tr!(
                "Не найдена папка AppData\\Local",
                "AppData\\Local folder not found"
            )
        )
    })?;
    Ok(base.join("claude-ru"))
}

/// Сборка `.dev` (идентификатор приложения) — выставляется один раз при старте.
static DEV_BUILD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_flavor(identifier: &str) {
    DEV_BUILD.store(
        identifier.ends_with(".dev"),
        std::sync::atomic::Ordering::Relaxed,
    );
}

/// Своя папка Claude этого экземпляра шлюза: профиль `Claude-3p`, русские копии. У release,
/// dev-сборки и изолированного запуска (`CLAUDE_GATEWAY_HOME`) она разная — иначе они
/// перезаписывали бы друг у друга запись `configLibrary` (порт и токен) и делили бы замок профиля.
pub fn base_dir() -> Option<PathBuf> {
    base_dir_for(
        std::env::var_os("CLAUDE_GATEWAY_HOME"),
        DEV_BUILD.load(std::sync::atomic::Ordering::Relaxed),
        local_appdata(),
    )
}

fn base_dir_for(
    home: Option<std::ffi::OsString>,
    dev: bool,
    local: Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(home) = home.filter(|v| !v.is_empty()) {
        // Абсолютный путь: Claude получает его в `--user-data-dir` и запускается в другой папке.
        let home = PathBuf::from(home);
        let home = std::path::absolute(&home).unwrap_or(home);
        return Some(home.join("claude"));
    }
    let name = if dev {
        "ClaudeProviderGateway-dev"
    } else {
        "ClaudeProviderGateway"
    };
    Some(local?.join(name))
}

/// AppData\Local: системная папка Windows, а не переменная окружения — та бывает
/// подменённой, если приложение запустили из процесса старой копии Claude.
pub fn local_appdata() -> Option<PathBuf> {
    dirs::data_local_dir().or_else(|| std::env::var_os("LOCALAPPDATA").map(PathBuf::from))
}

/// Модели для 3p-конфига Claude. С уровнями рассуждения — `behavesAs` и `maxEffort`
/// (схема `inferenceModels` в Claude 6.6395.0). Выбранные роли — `anthropicFamilyTier`
/// и `isFamilyDefault`: по ним Desktop задаёт своим сессиям Code
/// `ANTHROPIC_DEFAULT_{SONNET,OPUS,HAIKU}_MODEL`. Роль `fable` сюда не идёт: вторая запись
/// той же модели задвоила бы её в выборе модели. Одна модель — одна роль (у записи одно
/// поле): выбрана на несколько — достаётся первой из sonnet, opus, haiku, а остальные
/// Desktop решает сам, как без выбора.
fn inference_models(
    models: &[crate::model::ModelInfo],
    roles: &RoleAliases,
) -> Vec<serde_json::Value> {
    models
        .iter()
        .map(|model| {
            let mut row = json!({
                "name": model.alias,
                "labelOverride": format!(
                    "{} · {}",
                    model.label.as_deref().unwrap_or(&model.model),
                    model.provider
                ),
                "supports1m": model.supports_1m
            });
            let tier = [
                ("sonnet", &roles.sonnet),
                ("opus", &roles.opus),
                ("haiku", &roles.haiku),
            ]
            .into_iter()
            .find(|(_, alias)| alias.as_deref() == Some(model.alias.as_str()))
            .map(|(tier, _)| tier);
            if let Some(tier) = tier {
                row["anthropicFamilyTier"] = json!(tier);
                row["isFamilyDefault"] = json!(true);
            }
            if let Some(max) = &model.max_effort {
                // Anthropic-провайдер отдаёт настоящую модель Claude — она и есть образец.
                // Anthropic-совместимый endpoint может отдавать и не-Claude (DeepSeek, GLM):
                // образцом служит сама модель, только если это модель Claude.
                let own = ctp_core::alias::strip_1m(&model.model).trim().to_string();
                let profile = if !model.openai && own.starts_with("claude-") {
                    own
                } else {
                    EFFORT_PROFILE.to_string()
                };
                row["behavesAs"] = json!(profile);
                row["maxEffort"] = json!(max);
            }
            row
        })
        .collect()
}

/// `deploymentMode: "3p"` в `claude_desktop_config.json`. Claude хранит в этом же
/// файле свои настройки (`preferences`, папки Cowork…) — их сохраняем, а не затираем.
fn set_deployment_mode(path: &Path) -> Result<(), String> {
    let mut config = match fs::read_to_string(path) {
        Ok(text) => {
            match serde_json::from_str::<serde_json::Value>(text.trim_start_matches('\u{feff}')) {
                Ok(value) if value.is_object() => value,
                _ => {
                    // Битый файл не теряем: копия рядом (своя на каждый случай), пишем заново.
                    let stamp = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, |d| d.as_secs());
                    let backup = path.with_extension(format!("json.broken-{stamp}"));
                    fs::copy(path, &backup).map_err(|exc| {
                        let path = path.display();
                        tr!(
                            "{path} не читается, а копию сделать нельзя: {exc}",
                            "{path} is unreadable and cannot be backed up: {exc}"
                        )
                    })?;
                    json!({})
                }
            }
        }
        Err(exc) if exc.kind() == std::io::ErrorKind::NotFound => json!({}),
        // Занят, нет доступа, UTF-16… — не перезаписываем вслепую настройки Claude.
        Err(exc) => {
            let path = path.display();
            return Err(tr!(
                "Не прочитать {path}: {exc}",
                "Cannot read {path}: {exc}"
            ));
        }
    };
    if config["deploymentMode"] == "3p" {
        return Ok(());
    }
    config["deploymentMode"] = json!("3p");
    write_json(path, &config)
}

fn write_json(path: &Path, value: &serde_json::Value) -> Result<(), String> {
    let text = serde_json::to_string_pretty(value).map_err(|exc| exc.to_string())?;
    fs::write(path, text).map_err(|exc| format!("{}: {exc}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_folder_differs_per_build() {
        let local = || Some(PathBuf::from(r"C:\L"));
        let release = base_dir_for(None, false, local()).unwrap();
        assert_eq!(
            release,
            PathBuf::from(r"C:\L").join("ClaudeProviderGateway")
        );
        let dev = base_dir_for(None, true, local()).unwrap();
        assert_eq!(
            dev,
            PathBuf::from(r"C:\L").join("ClaudeProviderGateway-dev")
        );
        // Пустое значение — как будто переменной нет.
        assert_eq!(base_dir_for(Some("".into()), false, local()), Some(release));
        let home = base_dir_for(Some("rel-home".into()), false, local()).unwrap();
        assert!(
            home.is_absolute() && home.ends_with("rel-home/claude"),
            "{home:?}"
        );
        assert_eq!(base_dir_for(None, false, None), None);
    }
    use crate::model::ModelInfo;

    fn info(model: &str, max: Option<&str>, openai: bool) -> ModelInfo {
        ModelInfo {
            alias: format!("claude-p-{model}"),
            model: model.into(),
            provider: "P".into(),
            supports_1m: false,
            max_effort: max.map(str::to_string),
            openai,
            label: None,
        }
    }

    #[test]
    fn library_entry_carries_the_browser_switch() {
        let status = UiStatus {
            running: true,
            port: 17899,
            token: "tok".into(),
            models: vec![info("claude-p-1", None, true)],
            log_path: String::new(),
            claude: None,
            update: None,
        };
        let mut config = crate::model::Config::default();
        assert_eq!(
            library_entry(&config, &status, &[]).get("builtinBrowserEnabled"),
            Some(&serde_json::Value::Bool(false)),
            "по умолчанию браузер в 3p выключен"
        );
        config.builtin_browser = true;
        let entry = library_entry(&config, &status, &[]);
        assert_eq!(
            entry["builtinBrowserEnabled"],
            serde_json::Value::Bool(true)
        );
        assert_eq!(entry["inferenceGatewayBaseUrl"], "http://127.0.0.1:17899");
    }

    #[test]
    fn code_env_points_cli_at_gateway_and_drops_host_session() {
        let status = UiStatus {
            running: true,
            port: 17899,
            token: "tok".into(),
            models: vec![info("claude-p-1", None, true)],
            log_path: String::new(),
            claude: None,
            update: None,
        };
        let inherited = [
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_BASE_URL",
            "CLAUDE_CODE_SESSION_ID",
            "CLAUDE_CODE_ENTRYPOINT",
            "CLAUDE_PID",
            "CLAUDE_CODE_USER_EMAIL",
            "CLAUDE_CODE_DISABLE_AUTO_MEMORY",
            "PATH",
            "CLAUDE_CODE_USE_BEDROCK",
            "CLAUDECODE",
            "CLAUDE_CODE_SIMPLE",
            "MCP_TIMEOUT",
        ];
        let none = RoleAliases::default();
        let (remove, set) =
            code_env(&status, &none, inherited.iter().map(|s| s.to_string())).unwrap();
        assert_eq!(remove.len(), 9, "{remove:?}");
        assert!(
            !remove.contains(&"MCP_TIMEOUT".to_string()),
            "настройка пользователя"
        );
        assert!(!remove.contains(&"PATH".to_string()));
        // Пользовательская настройка Claude Code — не метка сессии, остаётся.
        assert!(!remove.contains(&"CLAUDE_CODE_DISABLE_AUTO_MEMORY".to_string()));
        let get = |k: &str| set.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        assert_eq!(get("ANTHROPIC_BASE_URL"), Some("http://127.0.0.1:17899"));
        assert_eq!(get("ANTHROPIC_AUTH_TOKEN"), Some("tok"));
        assert_eq!(get("ANTHROPIC_MODEL"), Some("claude-p-claude-p-1"));
        // Без выбора ролей — как до появления настройки: haiku на первой модели, а
        // sonnet/opus не задаются (встроенный ID → настоящая модель Claude или основная).
        assert_eq!(
            get("ANTHROPIC_DEFAULT_HAIKU_MODEL"),
            Some("claude-p-claude-p-1")
        );
        assert!(get("ANTHROPIC_DEFAULT_SONNET_MODEL").is_none());
        assert!(get("ANTHROPIC_DEFAULT_OPUS_MODEL").is_none());
        assert!(get("ANTHROPIC_API_KEY").is_none());
        assert_eq!(get("CLAUDE_CODE_MAX_CONTEXT_TOKENS"), Some("200000"));
        let mut big = status.clone();
        big.models[0].supports_1m = true;
        let (_, set_big) = code_env(&big, &none, std::iter::empty()).unwrap();
        assert!(set_big.contains(&("CLAUDE_CODE_MAX_CONTEXT_TOKENS".into(), "1000000".into())));

        let empty = UiStatus {
            models: Vec::new(),
            ..status
        };
        assert!(code_env(&empty, &none, std::iter::empty()).is_err());

        // Путь к claude в командную строку не попадает — только переменная.
        let line = code_command_line();
        assert_eq!(
            line,
            r#"/c start "Claude Code" cmd.exe /k ""%^CTP_CLAUDE_CLI%"""#
        );
    }

    /// Настоящее окно Claude Code через уже запущенный шлюз:
    /// `CTP_CODE_PORT=… CTP_CODE_TOKEN=… CTP_CODE_ALIAS=… CTP_CODE_DIR=… cargo test -p ctp-app launch_code_real -- --ignored`
    #[test]
    #[ignore]
    fn launch_code_real() {
        let var = |n: &str| std::env::var(n).expect(n);
        let status = UiStatus {
            running: true,
            port: var("CTP_CODE_PORT").parse().unwrap(),
            token: var("CTP_CODE_TOKEN"),
            models: vec![ModelInfo {
                alias: var("CTP_CODE_ALIAS"),
                ..info("m", None, true)
            }],
            log_path: String::new(),
            claude: None,
            update: None,
        };
        let config = Config {
            code_dir: Some(var("CTP_CODE_DIR")),
            ..Config::default()
        };
        println!("{}", launch_code(&config, &status).unwrap());
    }

    #[test]
    fn deployment_mode_keeps_claude_preferences() {
        let dir = std::env::temp_dir()
            .join("agents")
            .join(format!("ctp-claude-cfg-{}", uuid::Uuid::new_v4().simple()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("claude_desktop_config.json");

        set_deployment_mode(&path).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&fs::read_to_string(&path).unwrap()).unwrap(),
            json!({"deploymentMode": "3p"})
        );

        fs::write(&path, r#"{"deploymentMode":"1p","preferences":{"sidebarMode":"epitaxy"},"coworkUserFilesPath":"C:\\x"}"#).unwrap();
        set_deployment_mode(&path).unwrap();
        let saved: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["deploymentMode"], "3p");
        assert_eq!(saved["preferences"]["sidebarMode"], "epitaxy");
        assert_eq!(saved["coworkUserFilesPath"], "C:\\x");

        // BOM (Блокнот) — настройки сохраняются, а не уходят в «битые».
        fs::write(&path, "\u{feff}{\"deploymentMode\":\"1p\",\"keep\":1}").unwrap();
        set_deployment_mode(&path).unwrap();
        let saved: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["keep"], 1);

        fs::write(&path, "{broken").unwrap();
        set_deployment_mode(&path).unwrap();
        let backups: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".json.broken-"))
            .collect();
        assert_eq!(backups.len(), 1);
        assert_eq!(fs::read_to_string(backups[0].path()).unwrap(), "{broken");

        // Нечитаемый файл (UTF-16 из PowerShell 5) — ошибка, а не перезапись.
        fs::write(&path, [0xFF, 0xFE, b'{', 0, b'}', 0]).unwrap();
        assert!(set_deployment_mode(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), [0xFF, 0xFE, b'{', 0, b'}', 0]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn effort_fields_only_for_providers_with_levels() {
        let none = RoleAliases::default();
        let rows = inference_models(
            &[
                info("deepseek", Some("max"), true),
                info("gpt", None, true),
                info("claude-sonnet-4-6[1m]", Some("high"), false),
                info("glm-5", Some("max"), false),
            ],
            &none,
        );
        assert!(
            rows.iter().all(|r| r.get("anthropicFamilyTier").is_none()),
            "без выбора ролей конфиг как раньше"
        );
        assert_eq!(
            rows[3]["behavesAs"], EFFORT_PROFILE,
            "не-Claude за Anthropic API"
        );
        assert_eq!(rows[0]["behavesAs"], EFFORT_PROFILE);
        assert_eq!(rows[0]["maxEffort"], "max");
        assert!(rows[1].get("behavesAs").is_none() && rows[1].get("maxEffort").is_none());
        assert_eq!(rows[2]["behavesAs"], "claude-sonnet-4-6");
        assert_eq!(rows[2]["maxEffort"], "high");
        assert_eq!(rows[0]["labelOverride"], "deepseek · P");
        let mut named = info("deepseek-v4-pro", None, true);
        named.label = Some("DeepSeek V4 Pro".into());
        assert_eq!(
            inference_models(&[named], &none)[0]["labelOverride"],
            "DeepSeek V4 Pro · P"
        );
    }

    fn status_with(models: Vec<ModelInfo>) -> UiStatus {
        UiStatus {
            running: true,
            port: 17899,
            token: "tok".into(),
            models,
            log_path: String::new(),
            claude: None,
            update: None,
        }
    }

    #[test]
    fn advisor_is_available_only_on_an_openai_strong_model() {
        let row = |id: &str, kind: &str| crate::model::ProviderRow {
            id: id.into(),
            name: id.into(),
            kind: kind.into(),
            base_url: "https://example.com/v1".into(),
            models: vec!["m1".into()],
            ..Default::default()
        };
        let mut config = Config {
            providers: vec![row("oa", "openai"), row("an", "anthropic")],
            ..Default::default()
        };
        assert!(!advisor_available(&config));
        let strong = |provider: &str, model: &str| {
            Some(crate::model::ModelRef {
                provider: provider.into(),
                model: model.into(),
            })
        };
        config.model_roles.opus = strong("oa", "m1");
        assert!(advisor_available(&config));
        config.model_roles.opus = strong("an", "m1");
        assert!(!advisor_available(&config));
        // Модели больше нет у провайдера — как не выбрана.
        config.model_roles.opus = strong("oa", "gone");
        assert!(!advisor_available(&config));
    }

    #[test]
    fn roles_go_to_cli_env_and_desktop_tiers() {
        let mut fast = info("fast", None, true);
        fast.supports_1m = true;
        let mut strong = info("strong", None, true);
        strong.supports_1m = true;
        let main = info("main", None, true);
        let status = status_with(vec![fast.clone(), main.clone(), strong.clone()]);
        let roles = RoleAliases {
            sonnet: Some(main.alias.clone()),
            opus: Some(strong.alias.clone()),
            haiku: Some(fast.alias.clone()),
        };
        let (_, set) = code_env(&status, &roles, std::iter::empty()).unwrap();
        let get = |k: &str| set.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        assert_eq!(get("ANTHROPIC_MODEL"), Some(main.alias.clone()));
        assert_eq!(
            get("ANTHROPIC_DEFAULT_SONNET_MODEL"),
            Some(main.alias.clone())
        );
        assert_eq!(
            get("ANTHROPIC_DEFAULT_OPUS_MODEL"),
            Some(strong.alias.clone())
        );
        assert_eq!(
            get("ANTHROPIC_DEFAULT_HAIKU_MODEL"),
            Some(fast.alias.clone())
        );
        assert_eq!(get("ANTHROPIC_SMALL_FAST_MODEL"), Some(fast.alias.clone()));
        // fable (советники) — на сильную, как opus; советник Claude Code включён.
        assert_eq!(
            get("ANTHROPIC_DEFAULT_FABLE_MODEL"),
            Some(strong.alias.clone())
        );
        assert_eq!(get(ADVISOR_ENV), Some("1".into()));
        // Окно — по основной модели (без 1M), даже если у быстрой и сильной 1M.
        assert_eq!(get("CLAUDE_CODE_MAX_CONTEXT_TOKENS"), Some("200000".into()));

        // Только opus выбран: основная и haiku — первая модель.
        let only_opus = RoleAliases {
            opus: Some(strong.alias.clone()),
            ..Default::default()
        };
        let (_, set) = code_env(&status, &only_opus, std::iter::empty()).unwrap();
        let get = |k: &str| set.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        assert_eq!(get("ANTHROPIC_MODEL"), Some(fast.alias.clone()));
        assert_eq!(
            get("ANTHROPIC_DEFAULT_HAIKU_MODEL"),
            Some(fast.alias.clone())
        );
        assert_eq!(
            get("ANTHROPIC_DEFAULT_OPUS_MODEL"),
            Some(strong.alias.clone())
        );
        assert!(get("ANTHROPIC_DEFAULT_SONNET_MODEL").is_none());
        assert_eq!(
            get("CLAUDE_CODE_MAX_CONTEXT_TOKENS"),
            Some("1000000".into())
        );

        // Сильная на Anthropic-провайдере: fable — на неё, а советника нет (шлюз его там
        // не исполняет, и снимать запрет на беты в копии незачем).
        let anthropic = info("anth", None, false);
        let mixed = status_with(vec![main.clone(), anthropic.clone()]);
        let on_anthropic = RoleAliases {
            opus: Some(anthropic.alias.clone()),
            ..Default::default()
        };
        let (_, set) = code_env(&mixed, &on_anthropic, std::iter::empty()).unwrap();
        let get = |k: &str| set.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        assert_eq!(
            get("ANTHROPIC_DEFAULT_FABLE_MODEL"),
            Some(anthropic.alias.clone())
        );
        assert!(get(ADVISOR_ENV).is_none());

        // Модель роли не запущена (шлюз со старым набором) — роль как не выбранная.
        let stale = RoleAliases {
            sonnet: Some("claude-gone".into()),
            ..Default::default()
        };
        let (_, set) = code_env(&status, &stale, std::iter::empty()).unwrap();
        assert!(set.contains(&("ANTHROPIC_MODEL".into(), fast.alias.clone())));
        assert!(
            !set.iter()
                .any(|(n, _)| n == "ANTHROPIC_DEFAULT_FABLE_MODEL" || n == ADVISOR_ENV)
        );

        // Desktop: роль — в записи модели; одна модель на две роли — первая из sonnet, opus.
        let rows = inference_models(&status.models, &roles);
        assert_eq!(rows[0]["anthropicFamilyTier"], "haiku");
        assert_eq!(rows[1]["anthropicFamilyTier"], "sonnet");
        assert_eq!(rows[2]["anthropicFamilyTier"], "opus");
        assert!(rows.iter().all(|r| r["isFamilyDefault"] == true));
        // fable в Desktop не дублируется (советника шлюз ведёт к сильной модели сам).
        assert_eq!(rows.len(), 3);
        let shared = RoleAliases {
            sonnet: Some(main.alias.clone()),
            opus: Some(main.alias.clone()),
            haiku: None,
        };
        let rows = inference_models(&status.models, &shared);
        assert_eq!(rows[1]["anthropicFamilyTier"], "sonnet");
        assert!(rows[0].get("anthropicFamilyTier").is_none());

        // Команда для своего терминала — из того же окружения.
        let text = code_command(&status, &roles).unwrap();
        assert!(text.contains(&format!(
            "$env:ANTHROPIC_DEFAULT_OPUS_MODEL=\"{}\"",
            strong.alias
        )));
        assert!(text.ends_with("\nclaude"));
    }
}
