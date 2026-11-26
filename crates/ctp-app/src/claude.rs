use crate::model::{Config, ModelInfo, UiStatus};
use crate::tr;
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Постоянный ID записи в `configLibrary`: Claude видит ту же конфигурацию от запуска к запуску.
const LIBRARY_ID: &str = "6b1f3c52-7a0e-4f7e-9d4b-c1a0de9a7e3d";

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
    if programs.is_file() {
        return Some(programs);
    }
    // Установщик Anthropic ставит MSIX и убирает Squirrel-папку (ADR-021).
    crate::msix::installed_exe()
}

fn version_key(name: &str) -> Vec<u64> {
    name.trim_start_matches("app-")
        .split('.')
        .filter_map(|part| part.parse().ok())
        .collect()
}

/// Копия установленной версии: с русским переводом или без него (тогда в ней только
/// настоящая AppData\Local для процессов Claude); любая ошибка — повод запустить оригинал.
fn claude_copy(
    exe: &Path,
    copies: &Path,
    config: &Config,
    config_dir: &Path,
) -> anyhow::Result<PathBuf> {
    use anyhow::Context;
    let (ru, catalog) = crate::upkeep::copy_translation(config, config_dir)?;
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
    // Собирать копию из версии, которую установщик ещё распаковывает, нельзя: недостроенная
    // копия по своей метке считалась бы готовой. Пока папка не успокоилась — оригинал.
    anyhow::ensure!(
        crate::ruui::copy_state(app_dir, copies, &ru, &catalog)? == crate::ruui::CopyState::Current
            || crate::upkeep::settled(app_dir, crate::upkeep::SETTLE),
        "{}",
        tr!(
            "Claude {dir} ещё устанавливается — русская копия соберётся через пару минут",
            "Claude {dir} is still being installed; the Russian copy will be built in a couple of minutes"
        )
    );
    crate::ruui::ensure_copy(app_dir, copies, &ru, &catalog)
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

/// Псевдоним модели, выбранной основной (`None` — не выбрана или её больше нет у
/// провайдера).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RoleAliases {
    pub sonnet: Option<String>,
}

pub fn role_aliases(config: &Config) -> RoleAliases {
    RoleAliases {
        sonnet: crate::store::role_alias(config, config.model_roles.sonnet.as_ref()),
    }
}

/// Окно, которое Claude Code считает у основной модели: из каталога, но в тех же рамках, что
/// у `ollama launch claude` (меньше 100K сессия Claude Code не помещается, больше 1M он не
/// знает).
const MIN_WINDOW: u64 = 100_000;
const MAX_WINDOW: u64 = 1_000_000;
/// Потолок `CLAUDE_CODE_MAX_OUTPUT_TOKENS` для незнакомых Claude Code моделей.
const MAX_OUTPUT: u64 = 128_000;

/// Переменные Claude Code для любой его сессии через шлюз: в консоли кнопки «Claude Code» и
/// во вкладке Code у Claude Desktop (Desktop передаёт сессиям своё окружение).
pub const SESSION_ENV: [(&str, &str); 5] = [
    // Проверку auto mode делает классификатор самого Claude Code, а не сервер
    // (code.claude.com/docs/en/auto-mode-config).
    ("CLAUDE_CODE_AUTO_MODE_SERVER", "0"),
    // Псевдонимы шлюза Claude Code не узнаёт и для них выключает Task-инструменты
    // (в 2.1.331 — только по этой переменной).
    ("CLAUDE_CODE_ENABLE_TODO_TOOLS", "1"),
    // Инструменты — в списке `tools`, а не блоками внутри сообщений: такие блоки
    // Anthropic-эндпоинт DeepSeek отвергает с 422, и сессия не восстанавливается
    // (anthropics/claude-code#95087).
    ("CLAUDE_CODE_INLINE_TOOLS", "false"),
    // Блок атрибуции Anthropic вырезает у себя, а модель провайдера видела бы его в начале
    // системного промпта.
    ("CLAUDE_CODE_ATTRIBUTION_HEADER", "0"),
    // Коннекторы claude.ai требуют входа в аккаунт Anthropic — через шлюз их нет.
    ("ENABLE_CLAUDEAI_MCP_SERVERS", "false"),
];

/// Основная модель шлюза: роль `sonnet` или первая. Роль, чьей модели нет среди запущенных
/// (шлюз не перезапускали), — как не выбранная.
pub fn main_model<'a>(status: &'a UiStatus, roles: &RoleAliases) -> Option<&'a ModelInfo> {
    roles
        .sonnet
        .as_ref()
        .and_then(|alias| status.models.iter().find(|m| &m.alias == alias))
        .or_else(|| status.models.first())
}

/// Имя модели для Claude Code: настоящий ID, если он в шлюзе у одной модели, иначе псевдоним.
fn cli_model_name<'a>(status: &'a UiStatus, model: &'a ModelInfo) -> &'a str {
    let unique = status
        .models
        .iter()
        .filter(|m| m.model == model.model)
        .count()
        == 1;
    if unique { &model.model } else { &model.alias }
}

/// Переменные модели-критика (ADR-012). Последняя выбирает путь жёстко: иначе критик сам
/// угадывает подписку по файлу входа, который мог остаться от старого входа в Claude.
pub const CRITIC_ENV: [&str; 4] = [
    "AGENT_KIT_PGC_URL",
    "AGENT_KIT_PGC_TOKEN",
    "AGENT_KIT_PGC_MODEL",
    "AGENT_KIT_CRITIC_VIA",
];

/// Модель-критик для скриптов проекта: адрес шлюза, его токен и модель. Пусто, если она не
/// выбрана, шлюз стоит или её нет среди запущенных моделей (шлюз не перезапускали).
pub fn critic_env(config: &Config, status: &UiStatus) -> Vec<(String, String)> {
    let model = crate::store::role_alias(config, config.model_roles.critic.as_ref())
        .and_then(|alias| status.models.iter().find(|m| m.alias == alias))
        .filter(|_| status.running);
    let Some(model) = model else {
        return Vec::new();
    };
    let values = [
        format!("http://127.0.0.1:{}", status.port),
        status.token.clone(),
        cli_model_name(status, model).to_string(),
        "pgc".to_string(),
    ];
    CRITIC_ENV
        .iter()
        .map(|name| name.to_string())
        .zip(values)
        .collect()
}

/// Окно, которое PGC задаёт консоли: из каталога, нет его — 1M (см. `code_env`).
fn console_window(limits: &crate::catalog::Hint) -> u64 {
    limits
        .context
        .map_or(MAX_WINDOW, |context| context.clamp(MIN_WINDOW, MAX_WINDOW))
}

/// Окружение Claude Code через шлюз: что убрать и что задать. Основная модель — роль
/// `sonnet` или первая модель шлюза; на неё же все роли Claude Code (opus, haiku, fable).
/// `limits` — окно и предел вывода основной модели из каталога, если они известны.
pub fn code_env(
    status: &UiStatus,
    roles: &RoleAliases,
    inherited: impl Iterator<Item = String>,
    limits: &crate::catalog::Hint,
) -> Result<CodeEnv, String> {
    let main = main_model(status, roles).ok_or_else(|| {
        tr!(
            "В шлюзе нет моделей: добавьте провайдера и модель",
            "The gateway has no models: add a provider and a model"
        )
    })?;
    // Имя шлюза Claude Code не знает и считает окно 1M — у модели с окном меньше сжатие
    // разговора началось бы слишком поздно. Окно из каталога; нет его — как в Claude
    // Desktop: без `[1m]` 200K. По основной модели: на ней сессия и наследующие её
    // субагенты (Explore, Plan).
    // Окно неизвестно — 1M, как сам Claude Code считает у незнакомых моделей. Завышенное окно
    // безопасно: переполнение провайдера шлюз отдаёт как «prompt is too long», и Claude Code
    // сжимает разговор. Заниженное — нет: если его почти целиком занимают инструменты MCP,
    // сжимать нечего, и сессия сжимается по кругу (видели вживую на 200K и 626 инструментах).
    let window = console_window(limits);
    let remove = inherited
        .filter(|name| {
            let upper = name.to_ascii_uppercase();
            HOST_ENV_PREFIXES.iter().any(|p| upper.starts_with(p))
        })
        .collect();
    // Заголовок и строку «Default» Claude Code подписывает самим ID модели. Псевдоним
    // `claude-…` нужен только фильтру имён Desktop (ADR-005), а шлюз понимает и настоящий
    // ID — консоль получает его, как у `ollama launch claude`. Тот же ID у двух моделей
    // шлюз развёл бы неоднозначно — тогда псевдоним.
    let id = &cli_model_name(status, main).to_string();
    let mut set: Vec<(String, String)> = vec![
        (
            "ANTHROPIC_BASE_URL".into(),
            format!("http://127.0.0.1:{}", status.port),
        ),
        ("ANTHROPIC_AUTH_TOKEN".into(), status.token.clone()),
        ("ANTHROPIC_MODEL".into(), id.clone()),
    ];
    // Каждая роль — на основную, с её именем в `/model`. Роль без переменной Claude Code
    // показал бы встроенной моделью Anthropic с ценой (Opus 5.5, Fable 5.2), которой
    // через шлюз нет, а без `_NAME` — голым псевдонимом шлюза.
    let name = format!(
        "{} · {}",
        main.label.as_deref().unwrap_or(&main.model),
        main.provider
    );
    for (role, title) in [
        ("SONNET", "Sonnet"),
        ("OPUS", "Opus"),
        ("HAIKU", "Haiku"),
        ("FABLE", "Fable"),
    ] {
        let var = format!("ANTHROPIC_DEFAULT_{role}_MODEL");
        set.extend([
            (format!("{var}_NAME"), name.clone()),
            (
                format!("{var}_DESCRIPTION"),
                tr!(
                    "Основная модель PGC (вместо {title})",
                    "PGC main model (in place of {title})"
                ),
            ),
            (var, id.clone()),
        ]);
    }
    set.extend([
        // Устаревшее имя роли haiku: его ещё читают старые версии Claude Code.
        ("ANTHROPIC_SMALL_FAST_MODEL".into(), id.clone()),
        ("CLAUDE_CODE_MAX_CONTEXT_TOKENS".into(), window.to_string()),
        // Провайдер задаёт запуск: `env` из settings.json человека не уведёт Claude Code
        // мимо шлюза (как `ollama launch claude`). Desktop своим сессиям задаёт его сам.
        ("CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST".into(), "1".into()),
        // Инструменты MCP — по требованию (ADR-007). В Desktop это ключ `toolSearchEnabled`.
        ("ENABLE_TOOL_SEARCH".into(), "true".into()),
    ]);
    set.extend(
        SESSION_ENV
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string())),
    );
    // Окна 1M у модели нет, а строка «Default» в `/model` даёт её ID с `[1m]`: Claude Code
    // тогда считает окно 1M и `CLAUDE_CODE_MAX_CONTEXT_TOKENS` не применяет (проверено
    // вживую на 2.1.331). Эта переменная снимает значение `[1m]`.
    if window < MAX_WINDOW {
        set.push(("CLAUDE_CODE_DISABLE_1M_CONTEXT".into(), "1".into()));
    }
    // Без него Claude Code просит у незнакомой модели 32000 токенов вывода: у одних это выше
    // их предела, у других обрезает длинные ответы.
    if let Some(output) = limits.output {
        set.push((
            "CLAUDE_CODE_MAX_OUTPUT_TOKENS".into(),
            output.min(MAX_OUTPUT).to_string(),
        ));
    }
    Ok((remove, set))
}

/// Какой `claude` запускать и с какими переменными сверх шлюзовых: портативный — со
/// своей папкой настроек и без самообновления (его обновляет PGC); плюс прокси.
pub fn code_cli(config: &Config) -> Result<(PathBuf, Vec<(String, String)>), String> {
    let mut extra = crate::installs::proxy_env();
    if !config.code_portable {
        let cli = installed_code_cli().ok_or_else(|| {
            tr!(
                "Claude Code не найден: установите его (claude.ai/install), скачайте в Настройках или добавьте claude в PATH",
                "Claude Code not found: install it (claude.ai/install), download it in the settings or add claude to PATH"
            )
        })?;
        return Ok((cli, extra));
    }
    let (Some(cli), Some(dir)) = (
        crate::installs::portable_code(),
        crate::installs::portable_code_config(),
    ) else {
        return Err(tr!(
            "Портативный Claude Code не найден — скачайте его в Настройках",
            "Portable Claude Code not found; download it in the settings"
        ));
    };
    extra.push(("CLAUDE_CONFIG_DIR".into(), dir.display().to_string()));
    extra.push(("DISABLE_AUTOUPDATER".into(), "1".into()));
    Ok((cli, extra))
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
pub fn installed_code_cli() -> Option<PathBuf> {
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
pub fn launch_code(
    config: &Config,
    status: &UiStatus,
    limits: &crate::catalog::Hint,
) -> Result<String, String> {
    if !status.running {
        return Err(tr!("Сначала запустите шлюз", "Start the gateway first"));
    }
    // vars_os: значение не в Unicode не должно ронять запуск — нужны только имена.
    let (remove, mut set) = code_env(
        status,
        &role_aliases(config),
        std::env::vars_os().map(|(k, _)| k.to_string_lossy().into_owned()),
        limits,
    )?;
    // Окно сжатия из настроек PGC (ADR-019). Переменная приоритетнее `settings.json`,
    // так что работает и там, где ключ модели Claude Code читает неверно.
    if let Some(tokens) = crate::compact::console_tokens(config, console_window(limits)) {
        set.push(("CLAUDE_CODE_AUTO_COMPACT_WINDOW".into(), tokens.to_string()));
    }
    let (cli, extra) = code_cli(config)?;
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
    // Адрес с секретом браузерного эндпоинта нужен только шлюзу, не Claude Code и его инструментам.
    command.env_remove(crate::fetch::URL_ENV);
    command.envs(set.iter().map(|(k, v)| (k, v)));
    command.envs(extra.iter().map(|(k, v)| (k, v)));
    // Унаследованные значения критика — от другого запуска; задаём свои или никаких.
    for name in CRITIC_ENV {
        command.env_remove(name);
    }
    command.envs(critic_env(config, status));
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
    let mut entry = json!({
        "inferenceProvider": "gateway",
        "inferenceGatewayBaseUrl": format!("http://127.0.0.1:{}", status.port),
        "inferenceGatewayApiKey": status.token,
        "inferenceGatewayAuthScheme": "x-api-key",
        "inferenceCredentialKind": "static",
        "deploymentDisplayName": "Provider Gateway",
        "inferenceModels": models,
        // Серверную проверку действий auto mode (`safeguards`) ни один провайдер шлюза не
        // делает: без этого ключа каждый ход auto mode падает. Так Desktop отдаёт проверку
        // встроенному классификатору Claude Code (claude.com/docs/third-party/claude-desktop/gateway).
        "inferenceGatewayAutoModeServerChecks": false,
        // Как у Ollama (cmd/launch/claude_desktop.go): без списка Cowork режет плагинам и
        // MCP выход в сеть; телеметрию Anthropic профиль шлюза не шлёт.
        "coworkEgressAllowedHosts": ["*"],
        // Инструменты MCP — по требованию через ToolSearch, как в обычном Claude; шлюз не
        // отдаёт провайдеру отложенные, пока их не нашли (ADR-007). Без этого сотни схем
        // уходили в каждом запросе (~150–180K токенов).
        "toolSearchEnabled": true,
        "disableEssentialTelemetry": true,
        "disableNonessentialTelemetry": true,
    });
    // В 3p встроенный браузер выключен по умолчанию. Галочка в настройках только включает
    // его; без неё ключ не пишется, и действует то, что человек выбрал в окне настроек
    // Claude (иначе каждый запуск из PGC откатывал бы его выбор).
    if config.builtin_browser {
        entry["builtinBrowserEnabled"] = json!(true);
    }
    // Cowork в 3p идёт host-loop (см. ruui: снят msix_required). `standard` разрешает
    // Chat+Cowork; `off` оставляет Chat без VM-анализа, чтобы Cowork не включался.
    entry["desktopHome"] = json!(if config.cowork_mode == "off" {
        "no-vm"
    } else {
        "standard"
    });
    entry["projectsEnabled"] = json!(true);
    if config.artifacts {
        entry["artifactsEnabled"] = json!(true);
        entry["artifactsDesignEnabled"] = json!(true);
        entry["artifactsSlidesEnabled"] = json!(true);
    }
    entry
}

/// Поле записи с ключом шлюза: Claude сам шифрует его в файле, сравнивать его нельзя.
const API_KEY_FIELD: &str = "inferenceGatewayApiKey";

/// Запись, которую PGC хочет видеть в профиле Claude при нынешних настройках и шлюзе.
fn desired_entry(config: &Config, status: &UiStatus) -> serde_json::Value {
    let models = inference_models(&status.models, &role_aliases(config));
    library_entry(config, status, &models)
}

/// Что окно PGC показывает про Claude в своём профиле.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct ClaudeRun {
    /// Claude открыт в профиле PGC.
    pub running: bool,
    /// Открыт, но настройки шлюза (порт, модели, флажки) с тех пор изменились: нужен
    /// перезапуск Claude.
    pub stale: bool,
}

/// Состояние для строки статуса в окне: только чтение, ничего не пишется и не копируется.
pub fn run_state(config: &Config, status: &UiStatus) -> ClaudeRun {
    let Some(base) = base_dir() else {
        return ClaudeRun::default();
    };
    // VM-режим (ADR-018) запускает Claude со стандартным профилем; остальное — с папкой PGC.
    let vm = (config.cowork_mode == "vm")
        .then(|| real_local_appdata().map(|local| local.join("Claude-3p")))
        .flatten();
    let Some(profile) = [Some(base.join("Claude-3p")), vm]
        .into_iter()
        .flatten()
        .find(|path| profile_in_use(path))
    else {
        return ClaudeRun::default();
    };
    // Остановленный шлюз с нашей записью не сравнить: «устарело» не утверждаем.
    let stale = status.running && {
        let path = profile
            .join("configLibrary")
            .join(format!("{LIBRARY_ID}.json"));
        let existing = fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(text.trim_start_matches('\u{feff}')).ok());
        merge_entry(existing, &desired_entry(config, status)).1
    };
    ClaudeRun {
        running: true,
        stale,
    }
}

/// Наши ключи поверх существующей записи: то, что человек менял в окне Claude, остаётся.
/// Второе значение — изменились ли наши ключи (кроме зашифрованного ключа шлюза).
fn merge_entry(
    existing: Option<serde_json::Value>,
    entry: &serde_json::Value,
) -> (serde_json::Value, bool) {
    let ours = entry.as_object().cloned().unwrap_or_default();
    let Some(mut merged) = existing.filter(serde_json::Value::is_object) else {
        return (entry.clone(), true);
    };
    let changed = ours.iter().any(|(key, value)| {
        if key == API_KEY_FIELD {
            merged.get(key).is_none()
        } else {
            // Claude сам убирает из записи ключи со значением по умолчанию, поэтому
            // `false` в нашу запись не кладётся (см. `library_entry`).
            merged.get(key) != Some(value)
        }
    });
    if let Some(object) = merged.as_object_mut() {
        object.extend(ours);
    }
    (merged, changed)
}

/// Существующая запись настроек: `None` — файла нет. Нечитаемый как JSON-объект файл не
/// теряем: копия рядом, как у `claude_desktop_config.json`. Другая ошибка чтения — отказ:
/// вслепую поверх файла, в котором лежат настройки человека, не пишем.
fn read_entry(path: &Path) -> Result<Option<serde_json::Value>, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(exc) if exc.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(exc) => {
            let path = path.display();
            return Err(tr!(
                "Не прочитать {path}: {exc}",
                "Cannot read {path}: {exc}"
            ));
        }
    };
    match serde_json::from_str::<serde_json::Value>(text.trim_start_matches('\u{feff}')) {
        Ok(value) if value.is_object() => Ok(Some(value)),
        _ => {
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
            Ok(None)
        }
    }
}

/// Запись настроек Claude в `configLibrary`: наши ключи поверх существующей записи. Claude
/// уже открыт (`running`) — файл не трогаем, пока наши ключи те же: он сам зашифровал ключ
/// шлюза, и лишняя запись ничего не даст. `true` — наши ключи изменились.
fn sync_entry(
    library: &Path,
    entry: &serde_json::Value,
    running: bool,
    own_profile: bool,
) -> Result<bool, String> {
    let id = LIBRARY_ID;
    let entry_path = library.join(format!("{id}.json"));
    let (merged, changed) = merge_entry(read_entry(&entry_path)?, entry);
    if running && !changed {
        return Ok(false);
    }
    // Профиль наш собственный: старые записи (в них локальный токен) убираем, чтобы не
    // копились по одной на каждый запуск. Своя запись остаётся: в ней уцелеют ключи,
    // которые человек менял в окне Claude. В стандартном профиле (`vm`) там живут чужие
    // записи, их не трогаем, а в списке `_meta.json` оставляем.
    let meta_path = library.join("_meta.json");
    let mut entries = if own_profile {
        Vec::new()
    } else {
        read_entry(&meta_path)?
            .and_then(|meta| meta.get("entries").and_then(|e| e.as_array()).cloned())
            .unwrap_or_default()
    };
    if own_profile && let Ok(found) = fs::read_dir(library) {
        for other in found.flatten() {
            let path = other.path();
            let ours = path == entry_path || path.file_name().is_some_and(|n| n == "_meta.json");
            if !ours && path.extension().is_some_and(|ext| ext == "json") {
                let _ = fs::remove_file(path);
            }
        }
    }
    entries.retain(|e| e.get("id").and_then(|v| v.as_str()) != Some(id));
    entries.push(json!({"id": id, "name": "Provider Gateway"}));
    write_json(&entry_path, &merged)?;
    write_json(&meta_path, &json!({"entries": entries, "appliedId": id}))?;
    Ok(changed)
}

/// Запущен ли уже Claude с этим профилем. Пока он работает, Chromium держит `lockfile` в папке
/// профиля, а закрытый или упавший Claude его не оставляет (проверено): занятый файл — это
/// «работает».
pub(crate) fn profile_in_use(profile: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // ERROR_SHARING_VIOLATION: файл открыт другим процессом.
        const SHARING_VIOLATION: i32 = 32;
        match fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(profile.join("lockfile"))
        {
            Err(exc) => exc.raw_os_error() == Some(SHARING_VIOLATION),
            Ok(_) => false,
        }
    }
    #[cfg(not(windows))]
    {
        let _ = profile;
        false
    }
}

/// Ждёт, пока Claude закроется совсем: профиль свободен несколько опросов подряд. Один
/// свободный опрос ничего не значит — при самоперезапуске профиль на миг свободен.
fn wait_until_closed(mut is_open: impl FnMut() -> bool, poll: std::time::Duration, free: u32) {
    let mut quiet = 0;
    while quiet < free {
        std::thread::sleep(poll);
        quiet = if is_open() { 0 } else { quiet + 1 };
    }
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
    // Claude уже открыт в этом профиле: его не перезапускаем, русскую копию не пересобираем
    // (она занята), запись настроек меняем только если изменились наши ключи.
    // VM-режим: служба CoworkVMService принимает только exe из каталога пакета и хранит образ
    // по фиксированному пути `%LOCALAPPDATA%\Claude-3p\vm_bundles` (junction отвергает,
    // ADR-018). Поэтому профилем там служит стандартная папка, а не папка PGC.
    let mut vm_note = String::new();
    let msix = if config.cowork_mode == "vm" {
        let found = crate::msix::detect();
        if found.is_none() {
            vm_note = tr!(
                " MSIX Claude не найден — Cowork запущен на хосте, без VM.",
                " Claude MSIX not found; Cowork runs on the host without a VM."
            );
        } else if !crate::msix::service_installed() {
            vm_note = tr!(
                " Служба CoworkVMService не установлена — Cowork запущен на хосте, без VM.",
                " CoworkVMService is not installed; Cowork runs on the host without a VM."
            );
        }
        found.filter(|_| crate::msix::service_installed())
    } else {
        None
    };
    let vm = msix.is_some();
    let profile = if vm {
        real_local_appdata()
            .ok_or_else(|| {
                tr!(
                    "Не найдена папка AppData\\Local",
                    "AppData\\Local folder not found"
                )
            })?
            .join("Claude-3p")
    } else {
        base.join("Claude-3p")
    };
    let running = profile_in_use(&profile);
    if vm && config.russian_ui {
        vm_note.push_str(&tr!(
            " Русского перевода в этом режиме нет: пакетный Claude не переводится.",
            " There is no Russian translation in this mode: the packaged Claude is not translated."
        ));
    }
    let copies = base.join("claude-ru");
    let original = exe.clone();
    // Копия и без русского: только в ней процессы Claude (сессии Code, их npm, uv, git,
    // MCP) получают настоящую AppData\Local, а не папку PGC.
    let (exe, from_copy, note) = if running || vm {
        (exe, false, String::new())
    } else {
        match claude_copy(&exe, &copies, config, config_dir) {
            Ok(copy) if config.russian_ui => (
                copy,
                true,
                tr!(" Русский интерфейс включён.", " Russian interface is on."),
            ),
            Ok(copy) => (copy, true, String::new()),
            Err(exc) if config.russian_ui => {
                tracing::warn!(error = %format!("{exc:#}"), "Russian interface not applied");
                let note = tr!(
                    " Русский интерфейс не применён ({exc:#}) — запущен обычный Claude.",
                    " Russian interface not applied ({exc:#}); started the regular Claude."
                );
                (exe, false, note)
            }
            Err(exc) => {
                tracing::warn!(error = %format!("{exc:#}"), "Claude copy not built");
                let note = tr!(
                    " Копия Claude не собрана ({exc:#}) — запущен обычный Claude, его процессы видят папку PGC вместо AppData\\Local.",
                    " Claude copy not built ({exc:#}); started the regular Claude, its processes see PGC's folder instead of AppData\\Local."
                );
                (exe, false, note)
            }
        }
    };
    let library = profile.join("configLibrary");
    fs::create_dir_all(&library)
        .map_err(|exc| tr!("Профиль Claude: {exc}", "Claude profile: {exc}"))?;

    // Работающий Claude этот файл уже прочитал и сам его пишет; нечего трогать.
    if !running {
        set_deployment_mode(&profile.join("claude_desktop_config.json"))?;
    }

    let changed = sync_entry(&library, &desired_entry(config, status), running, !vm)?;

    let mut command = Command::new(&exe);
    // VM-режим: профиль стандартный, его путь служба вычисляет сама; ни аргумента, ни подмены
    // `LOCALAPPDATA` там быть не должно (ADR-018).
    if !vm {
        command
            .arg(format!("--user-data-dir={}", profile.display()))
            .env("LOCALAPPDATA", &base)
            .env("CLAUDE_USER_DATA_DIR", &profile);
    }
    command
        // Адрес с секретом браузерного эндпоинта нужен только шлюзу, не Claude и его инструментам.
        .env_remove(crate::fetch::URL_ENV)
        .current_dir(exe.parent().unwrap_or_else(|| Path::new(".")));
    // Настоящая AppData\Local для процессов, которые запускает копия (ruui::ENV_PRELUDE).
    // Из системной папки Windows, а не из окружения: приложение могли запустить из сессии
    // Claude, у которой оно само подменено.
    // В VM-режиме подмены нет, и прелюдии копии тоже нет — переменная не нужна.
    if !vm && let Some(real) = real_local_appdata() {
        command.env(crate::ruui::REAL_LOCALAPPDATA_VAR, real);
    }
    // Сессиям вкладки Code — те же переменные Claude Code, что и консоли: Desktop отдаёт им
    // своё окружение. Окно по модели здесь не задаётся: в Desktop модель переключают, его
    // даёт `behavesAs` каждой записи (`inference_models`).
    command.envs(SESSION_ENV);
    // Модель-критик для скриптов проектов в сессиях Code (ADR-012).
    for name in CRITIC_ENV {
        command.env_remove(name);
    }
    command.envs(critic_env(config, status));
    // Прокси приложения: сам Claude (Chromium, loopback он обходит сам) и сессии Code
    // (окружение). shortcut: смену прокси при открытом Claude окно не замечает как «изменились
    // настройки» — переоткрыть Claude подсказывает окно прокси; учесть в run_state, если
    // будут жаловаться.
    if let Some(proxy) = ctp_gateway::proxy::current() {
        command.arg(format!("--proxy-server={proxy}"));
        command.envs(crate::installs::proxy_env());
    }
    // VM-режим: настоящий exe пакета через Invoke-CommandInDesktopPackage (с профилем и
    // окружением PGC). Реестр не трогаем: запись делает MSIX-пакет.
    if let Some(package) = &msix {
        let args: Vec<String> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        let envs: Vec<(String, String)> = command
            .get_envs()
            .filter_map(|(name, value)| {
                value.map(|value| {
                    (
                        name.to_string_lossy().into_owned(),
                        value.to_string_lossy().into_owned(),
                    )
                })
            })
            .collect();
        crate::msix::launch(package, &args, &envs).map_err(|exc| format!("{exc:#}"))?;
        let profile = profile.display();
        let family = &package.family;
        return Ok(crate::model::LaunchResult {
            message: tr!(
                "Claude из MSIX (пакет {family}) запущен с профилем PGC: {profile}. Cowork с VM; русского перевода в этом режиме нет.",
                "Claude from MSIX (package {family}) started with the PGC profile: {profile}. Cowork with the VM; no Russian translation in this mode."
            ),
            warning: false,
        });
    }
    if running {
        // Второй запуск с тем же профилем только показывает первый экземпляр и выходит;
        // новых настроек он не подхватит — человек закрывает Claude сам, мы его не трогаем.
        command.spawn().map_err(|exc| {
            let exe = exe.display();
            tr!(
                "Не удалось запустить {exe}: {exc}",
                "Could not start {exe}: {exc}"
            )
        })?;
        return Ok(crate::model::LaunchResult {
            message: if changed {
                tr!(
                    "Claude уже открыт, а настройки шлюза изменились (порт, модели или флажки). Закройте Claude и откройте снова — иначе он останется со старыми.{vm_note}",
                    "Claude is already open, and the gateway settings have changed (port, models or switches). Close Claude and open it again, or it keeps the old ones.{vm_note}"
                )
            } else {
                tr!(
                    "Claude уже открыт в этом профиле — показан существующий.{vm_note}",
                    "Claude is already open in this profile; showing it.{vm_note}"
                )
            },
            warning: changed,
        });
    }
    // Снимок до запуска: копия перепишет `claude://` и COM-класс на себя (проверено вживую).
    // Портативный Claude пишет туда же свой путь — после закрытия его следа быть не должно.
    let portable =
        crate::installs::desktop_home().filter(|_| crate::installs::is_portable_desktop(&original));
    #[cfg(windows)]
    let before = from_copy.then(|| crate::ruui::registry::snapshot(&copies));
    #[cfg(windows)]
    let before_portable = portable
        .as_ref()
        .map(|home| crate::ruui::registry::snapshot(home));
    // Claude с профилем PGC (и портативный) при старте удаляет у установленного пункт «Open
    // in Claude Code» целиком: в этом профиле функции нет (проверено вживую). Возвращаем.
    #[cfg(windows)]
    let owned = crate::ruui::registry::capture(&crate::ruui::registry::OWNED);
    #[cfg(not(windows))]
    let (before, before_portable): (Option<Before>, Option<Before>) = (None, None);
    let child = command.spawn().map_err(|exc| {
        let exe = exe.display();
        tr!(
            "Не удалось запустить {exe}: {exc}",
            "Could not start {exe}: {exc}"
        )
    })?;
    let pid = child.id();
    // Возвращаем ключи установленному Claude после старта копии и ещё раз, когда она
    // закроется. Если окно шлюза закроют раньше — это сделает выход из приложения или
    // следующий старт (repair_registry, уже без снимка).
    {
        let locked = profile.clone();
        let clean = move || {
            if from_copy {
                repair_for(&copies, &original, before.as_ref());
            }
            if let Some(home) = &portable {
                forget_portable(home, before_portable.as_ref());
            }
            #[cfg(windows)]
            crate::ruui::registry::put_back(&owned);
        };
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(20));
            clean();
            // Не `child.wait()`: Claude перезапускает сам себя (после «Save & Restart» в своём
            // окне настроек), запущенный нами процесс выходит, а копия живёт дальше и снова
            // пишет в реестр свой путь. Закрытие — это свободный `lockfile` профиля.
            // Самоперезапуск — профиль был свободен и снова занят: ключи чистятся заново,
            // иначе до закрытия копии ссылки `claude://` открывали бы её.
            let mut was_free = false;
            wait_until_closed(
                || {
                    let open = profile_in_use(&locked);
                    if open && was_free {
                        std::thread::sleep(std::time::Duration::from_secs(20));
                        clean();
                    }
                    was_free = !open;
                    open
                },
                std::time::Duration::from_secs(5),
                2,
            );
            clean();
        });
    }
    let profile = profile.display();
    Ok(crate::model::LaunchResult {
        message: tr!(
            "Claude запущен (PID {pid}). Профиль: {profile}.{note}{vm_note}",
            "Claude started (PID {pid}). Profile: {profile}.{note}{vm_note}"
        ),
        // Открылся обычный Claude, а не копия (без перевода или без настоящей
        // AppData\Local у процессов) — окно покажет это как ошибку.
        warning: !from_copy || !vm_note.is_empty(),
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
    // Только для Claude в папке версии (`…\app-X\claude.exe` или `<package>\app\claude.exe`):
    // иначе «папка установки» вычисляется неверно (заглушка Squirrel, Programs).
    #[cfg(windows)]
    if let Some(app) = crate::upkeep::app_dir(original_exe) {
        let installed = app.parent().unwrap_or(&app);
        crate::ruui::registry::repair(copies, installed, &app, before);
    }
    #[cfg(not(windows))]
    let _ = (copies, original_exe, before);
}

/// Перед выходом из приложения: поток, что вернёт ключи после закрытия копии, умирает вместе
/// с ним, и пока копия открыта, её след в реестре остался бы до следующего старта
/// (`claude://` и клик по уведомлению обычного Claude открывали бы копию). Чинятся только
/// ключи установленного Claude, копия не трогается.
pub fn repair_registry_on_exit(app: &tauri::AppHandle) {
    use tauri::Manager;
    if let Some(shared) = app.try_state::<crate::state::AppShared>() {
        let config = shared
            .config
            .lock()
            .unwrap_or_else(|exc| exc.into_inner())
            .clone();
        repair_registry(&config);
    }
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
    if let Some(home) = crate::installs::desktop_home().filter(|home| home.is_dir()) {
        forget_portable(&home, None);
    }
}

/// След портативного Claude в реестре: если установлен обычный Claude — ключи
/// возвращаются ему (как после русской копии), иначе убираются совсем.
fn forget_portable(home: &Path, before: Option<&Before>) {
    let installed =
        detect(&Config::default()).filter(|exe| !crate::installs::is_portable_desktop(exe));
    match installed {
        Some(installed) => repair_for(home, &installed, before),
        #[cfg(windows)]
        None => crate::ruui::registry::forget(home, before),
        #[cfg(not(windows))]
        None => {}
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

pub fn is_dev_build() -> bool {
    DEV_BUILD.load(std::sync::atomic::Ordering::Relaxed)
}

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
/// (схема `inferenceModels` в Claude 6.6395.0). Основная — `anthropicFamilyTier: sonnet`
/// и `isFamilyDefault`: по ним Desktop задаёт своим сессиям Code
/// `ANTHROPIC_DEFAULT_SONNET_MODEL`; остальные роли Desktop решает сам, как без выбора.
fn inference_models(
    models: &[crate::model::ModelInfo],
    roles: &RoleAliases,
) -> Vec<serde_json::Value> {
    models
        .iter()
        .map(|model| {
            // Anthropic-провайдер отдаёт настоящую модель Claude, а Anthropic-совместимый
            // endpoint может отдавать и не-Claude (DeepSeek, GLM).
            let own = ctp_core::alias::strip_1m(&model.model).trim().to_string();
            let real_claude = !model.openai && own.starts_with("claude-");
            // Claude под чужим ID (`anthropic/claude-…`, `us.anthropic.claude-…`) — тоже Claude.
            let claude_inside = !model.openai && own.to_ascii_lowercase().contains("claude-");
            // Окно неизвестно и образца `behavesAs` нет (нет уровней рассуждения) — всё равно
            // 1M, как у остальных (ADR-005, ADR-011): иначе Desktop взял бы свои 200K.
            let window_1m = model.supports_1m || (model.max_effort.is_none() && !claude_inside);
            let mut row = json!({
                "name": model.alias,
                "labelOverride": format!(
                    "{} · {}",
                    model.label.as_deref().unwrap_or(&model.model),
                    model.provider
                ),
                "supports1m": window_1m
            });
            // Одна строка вместо двух («обычная» и «1 млн») у всех моделей с окном 1M: у
            // провайдера модель одна, а у Claude 5.x окно 1M своё и от строки меню не зависит
            // (ADR-011, исправление 25.11). Кто пометил модель `[1m]`, тот и хочет 1M.
            if window_1m {
                row["prefer1m"] = json!(true);
            }
            if roles.sonnet.as_deref() == Some(model.alias.as_str()) {
                row["anthropicFamilyTier"] = json!("sonnet");
                row["isFamilyDefault"] = json!(true);
            }
            if let Some(max) = &model.max_effort {
                // Настоящая модель Claude сама служит образцом.
                // `behavesAs` даёт сессиям Code ещё и окно образца — у Opus 4.8 1M. Для модели
                // с неизвестным окном это верно (см. `code_env`: заниженное окно с тяжёлыми
                // MCP сжимает сессию по кругу, завышенное прикрывает «prompt is too long»).
                let profile = if real_claude {
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

/// `deploymentMode: "3p"` в `claude_desktop_config.json`. Заодно убирается запись MCP-поиска
/// `PGC Web search`, которую писали ранние dev-сборки: её exe больше не MCP-сервер, и Claude
/// при каждом старте поднимал бы окно шлюза. Чужие серверы человека остаются.
///
/// И поверхность окна: `preferences.sidebarMode`. `code` и `epitaxy` (вкладка Code) PGC
/// переключает на `chat` — окно открывается на Claude/Chat, а не в Code (ADR-020). Явно
/// выбранный Chat и любое другое значение не трогаются.
fn set_deployment_mode(path: &Path) -> Result<(), String> {
    update_desktop_config(path, |config| {
        config["deploymentMode"] = json!("3p");
        let code_surface = config
            .get("preferences")
            .and_then(|prefs| prefs.get("sidebarMode"))
            .and_then(serde_json::Value::as_str)
            .is_some_and(|mode| mode == "code" || mode == "epitaxy");
        if code_surface {
            config["preferences"]["sidebarMode"] = json!("chat");
        }
        if let Some(servers) = config
            .get_mut("mcpServers")
            .and_then(serde_json::Value::as_object_mut)
        {
            // shift_remove: порядок серверов человека в файле не меняется.
            servers.shift_remove("PGC Web search");
        }
    })
}

/// Правка `claude_desktop_config.json`. Claude хранит в этом же файле свои настройки
/// (`preferences`, папки Cowork…) — их сохраняем, а не затираем; без изменений файл не пишется.
pub(crate) fn update_desktop_config(
    path: &Path,
    patch: impl FnOnce(&mut serde_json::Value),
) -> Result<(), String> {
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
    let before = config.clone();
    patch(&mut config);
    if config == before {
        return Ok(());
    }
    write_json(path, &config)
}

fn write_json(path: &Path, value: &serde_json::Value) -> Result<(), String> {
    let text = serde_json::to_string_pretty(value).map_err(|exc| exc.to_string())?;
    crate::store::write_file(path, &text)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Портативный Claude Desktop из `PGC_LIVE_HOME` (его кладёт `live_portable_downloads`) в
    /// изолированном профиле, с `--proxy-server` на свой слушатель: идут ли через него
    /// запросы. Окно Claude откроется на ~30 с; закрываются только процессы из портативной
    /// папки. `cargo test -p ctp-app live_portable_launch -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_portable_launch_goes_through_the_proxy() {
        use std::io::{BufRead, BufReader};
        let home = std::env::var("PGC_LIVE_HOME").expect("set PGC_LIVE_HOME");
        // SAFETY: до любых потоков, читающих окружение.
        unsafe { std::env::set_var("CLAUDE_GATEWAY_HOME", &home) };
        let exe = crate::installs::portable_desktop().expect("no portable Claude in PGC_LIVE_HOME");

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut line = String::new();
                let _ = BufReader::new(&stream).read_line(&mut line);
                log.lock().unwrap().push(line.trim().to_string());
            }
        });
        ctp_gateway::proxy::set(Some(format!("http://127.0.0.1:{port}")));

        let config = Config {
            claude_path: Some(exe.display().to_string()),
            token: "live-test".into(),
            ..Config::default()
        };
        let status = UiStatus {
            running: true,
            port: 9, // никакой шлюз не слушает: нужен только запуск
            token: "live-test".into(),
            models: vec![crate::model::ModelInfo {
                alias: "claude-live-1000".into(),
                model: "m".into(),
                provider: "p".into(),
                supports_1m: false,
                max_effort: None,
                openai: true,
                label: None,
            }],
            log_path: String::new(),
            claude: None,
            update: None,
        };
        let dir = std::path::PathBuf::from(&home);
        let result = launch(&config, &status, &dir).unwrap();
        println!("{}", result.message);
        std::thread::sleep(std::time::Duration::from_secs(30));
        let folder = exe.parent().unwrap().display().to_string();
        let kill = format!(
            "Get-Process claude -ErrorAction SilentlyContinue | Where-Object {{ $_.Path -like '{folder}*' }} | Stop-Process -Force"
        );
        let _ = Command::new("powershell.exe")
            .args(["-NoProfile", "-Command", &kill])
            .status();
        // Поток уборки реестра: два свободных опроса профиля по 5 с.
        std::thread::sleep(std::time::Duration::from_secs(20));
        ctp_gateway::proxy::set(None);
        let seen = seen.lock().unwrap().clone();
        println!("through the proxy: {seen:?}");
        assert!(
            seen.iter()
                .any(|l| l.starts_with("CONNECT ") || l.starts_with("GET http")),
            "Claude did not use --proxy-server"
        );
    }

    #[test]
    fn closing_needs_consecutive_free_polls_not_one() {
        // Claude restarts itself: for an instant the profile is free, and the registry must not be
        // repaired then (the new process writes its own path right after).
        let mut answers = vec![true, false, true, true, false, false].into_iter();
        let mut polls = 0;
        wait_until_closed(
            || {
                polls += 1;
                answers.next().unwrap()
            },
            std::time::Duration::from_millis(1),
            2,
        );
        assert_eq!(polls, 6);
    }

    #[test]
    fn our_keys_overwrite_the_entry_and_the_rest_of_it_survives() {
        let status = status_with(Vec::new());
        let config = Config::default();
        let ours = library_entry(&config, &status, &[]);
        // Нет записи (или она не объект) — пишем свою, это «изменилось».
        assert_eq!(merge_entry(None, &ours), (ours.clone(), true));
        assert!(merge_entry(Some(json!([1])), &ours).1);

        // Claude зашифровал ключ и добавил свою настройку: наши ключи те же — не изменилось,
        // чужое осталось, ключ шлюза снова открытый (Claude зашифрует его сам).
        let mut seen = ours.clone();
        seen["inferenceGatewayApiKey"] = json!("__sealed__:v1:xxx");
        seen["autoModeEnabled"] = json!(true);
        let (merged, changed) = merge_entry(Some(seen), &ours);
        assert!(!changed);
        assert_eq!(merged["autoModeEnabled"], true);
        assert_eq!(
            merged["inferenceGatewayApiKey"],
            ours["inferenceGatewayApiKey"]
        );

        // Изменился порт, флажок браузера или список моделей — изменилось.
        let mut moved = ours.clone();
        moved["inferenceGatewayBaseUrl"] = json!("http://127.0.0.1:1");
        assert!(merge_entry(Some(moved), &ours).1);
        // Браузер: без нашей галочки ключа в `ours` нет — чужое значение не «изменение».
        let mut chosen_in_claude = ours.clone();
        chosen_in_claude["builtinBrowserEnabled"] = json!(true);
        let (merged, changed) = merge_entry(Some(chosen_in_claude), &ours);
        assert!(!changed && merged["builtinBrowserEnabled"] == true);
        // С галочкой: нет ключа или он выключен — изменилось, уже включён — нет.
        let mut on = ours.clone();
        on["builtinBrowserEnabled"] = json!(true);
        assert!(merge_entry(Some(ours.clone()), &on).1);
        let mut off = ours.clone();
        off["builtinBrowserEnabled"] = json!(false);
        assert!(merge_entry(Some(off), &on).1);
        assert!(!merge_entry(Some(on.clone()), &on).1);
        // Записи без ключа шлюза — тоже изменилось.
        let mut keyless = ours.clone();
        keyless
            .as_object_mut()
            .unwrap()
            .remove("inferenceGatewayApiKey");
        assert!(merge_entry(Some(keyless), &ours).1);
    }

    #[test]
    fn the_entry_file_is_synced_without_touching_what_must_not_be_touched() {
        let dir = std::env::temp_dir()
            .join("agents")
            .join(format!("ctp-sync-{}", uuid::Uuid::new_v4().simple()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{LIBRARY_ID}.json"));
        let read = |p: &Path| {
            serde_json::from_str::<serde_json::Value>(&fs::read_to_string(p).unwrap()).unwrap()
        };
        let config = Config::default();
        let model = |name: &str| json!({"name": name, "supports1m": false});
        let ours = library_entry(&config, &status_with(Vec::new()), &[model("a")]);

        // Первый запуск: файла нет — пишем запись и `_meta.json`, чужие json убираем.
        fs::write(dir.join("old-random-id.json"), "{}").unwrap();
        assert!(sync_entry(&dir, &ours, false, true).unwrap());
        assert!(dir.join("_meta.json").is_file());
        assert!(!dir.join("old-random-id.json").exists());
        assert_eq!(read(&path)["inferenceModels"], json!([model("a")]));

        // Claude открыт, наши ключи те же: файл не переписывается (ключ остаётся зашифрованным).
        let mut sealed = read(&path);
        sealed["inferenceGatewayApiKey"] = json!("__sealed__:v1:xxx");
        sealed["autoModeEnabled"] = json!(true);
        fs::write(&path, serde_json::to_string(&sealed).unwrap()).unwrap();
        fs::write(dir.join("other.json"), "{}").unwrap();
        let before = fs::read_to_string(&path).unwrap();
        assert!(!sync_entry(&dir, &ours, true, true).unwrap());
        assert_eq!(fs::read_to_string(&path).unwrap(), before);
        assert!(
            dir.join("other.json").exists(),
            "a running Claude's folder is left alone"
        );

        // Claude открыт, список моделей стал длиннее: записано целиком, чужой ключ цел.
        let longer = library_entry(&config, &status_with(Vec::new()), &[model("a"), model("b")]);
        assert!(sync_entry(&dir, &longer, true, true).unwrap());
        let now = read(&path);
        assert_eq!(now["inferenceModels"], json!([model("a"), model("b")]));
        assert_eq!(now["autoModeEnabled"], true);
        assert_eq!(
            now["inferenceGatewayApiKey"],
            longer["inferenceGatewayApiKey"]
        );
        assert!(!dir.join("other.json").exists());

        // Битая запись: копия рядом, пишем свою.
        fs::write(&path, "{ not json").unwrap();
        assert!(sync_entry(&dir, &ours, false, true).unwrap());
        assert!(read(&path).is_object());
        let backups: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains("json.broken-"))
            .collect();
        assert_eq!(backups.len(), 1);
        // В копии — прежнее содержимое: ничего не потеряно.
        assert_eq!(fs::read_to_string(backups[0].path()).unwrap(), "{ not json");
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(windows)]
    #[test]
    fn a_held_lockfile_means_claude_is_running_in_the_profile() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = std::env::temp_dir()
            .join("agents")
            .join(format!("ctp-lock-{}", uuid::Uuid::new_v4().simple()));
        fs::create_dir_all(&dir).unwrap();
        // Нет файла — не запущен.
        assert!(!profile_in_use(&dir));
        let lock = dir.join("lockfile");
        let held = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .share_mode(0)
            .open(&lock)
            .unwrap();
        assert!(profile_in_use(&dir));
        // Закрыт (файл остался, но никто его не держит) — не запущен.
        drop(held);
        assert!(!profile_in_use(&dir));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn deployment_mode_keeps_claudes_own_settings_and_does_not_rewrite_a_finished_file() {
        let dir = std::env::temp_dir()
            .join("agents")
            .join(format!("ctp-dm-{}", uuid::Uuid::new_v4().simple()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("claude_desktop_config.json");
        // Старая запись MCP-поиска уходит, чужой MCP-сервер человека остаётся.
        fs::write(
            &path,
            r#"{"preferences":{"a":1},"mcpServers":{"first":{"command":"f"},"PGC Web search":{"command":"x"},"mine":{"command":"y"},"last":{"command":"l"}}}"#,
        )
        .unwrap();
        set_deployment_mode(&path).unwrap();
        let read = || {
            serde_json::from_str::<serde_json::Value>(&fs::read_to_string(&path).unwrap()).unwrap()
        };
        assert_eq!(read()["deploymentMode"], "3p");
        assert_eq!(read()["preferences"]["a"], 1);
        assert!(read()["mcpServers"]["PGC Web search"].is_null());
        assert_eq!(read()["mcpServers"]["mine"]["command"], "y");
        let order: Vec<String> = read()["mcpServers"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        assert_eq!(order, ["first", "mine", "last"]);
        // Уже 3p: файл не переписывается (метка времени и текст те же).
        let stamp = || fs::metadata(&path).unwrap().modified().unwrap();
        let (text, at) = (fs::read_to_string(&path).unwrap(), stamp());
        std::thread::sleep(std::time::Duration::from_millis(30));
        set_deployment_mode(&path).unwrap();
        assert_eq!((fs::read_to_string(&path).unwrap(), stamp()), (text, at));
        let _ = fs::remove_dir_all(&dir);
    }

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
        // Без серверной проверки auto mode: её не делает ни один провайдер шлюза.
        let entry = library_entry(&config, &status, &[]);
        assert_eq!(entry["inferenceGatewayAutoModeServerChecks"], false);
        // Как у Ollama: Cowork пускает плагины и MCP в сеть, телеметрии нет.
        assert_eq!(entry["coworkEgressAllowedHosts"], json!(["*"]));
        assert_eq!(entry["disableEssentialTelemetry"], true);
        assert_eq!(entry["disableNonessentialTelemetry"], true);
        // Инструменты MCP — по требованию (ADR-007).
        assert_eq!(entry["toolSearchEnabled"], true);
        let (_, env) = code_env(
            &status,
            &RoleAliases::default(),
            std::iter::empty(),
            &Default::default(),
        )
        .unwrap();
        for pair in [
            ("CLAUDE_CODE_AUTO_MODE_SERVER", "0"),
            ("CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST", "1"),
            ("CLAUDE_CODE_ENABLE_TODO_TOOLS", "1"),
            ("CLAUDE_CODE_INLINE_TOOLS", "false"),
            ("ENABLE_TOOL_SEARCH", "true"),
        ] {
            assert!(env.contains(&(pair.0.into(), pair.1.into())), "{pair:?}");
        }
        assert_eq!(
            library_entry(&config, &status, &[]).get("builtinBrowserEnabled"),
            None,
            "без галочки ключ не пишется: действует выбор человека в окне Claude"
        );
        config.builtin_browser = true;
        let entry = library_entry(&config, &status, &[]);
        assert_eq!(
            entry["builtinBrowserEnabled"],
            serde_json::Value::Bool(true)
        );
        assert_eq!(entry["inferenceGatewayBaseUrl"], "http://127.0.0.1:17899");
        // Cowork по умолчанию на хосте (ADR-018): Chat и Cowork вместе.
        assert_eq!(entry["desktopHome"], "standard");
        assert_eq!(entry["projectsEnabled"], true);
        assert_eq!(entry.get("artifactsEnabled"), None);
        config.artifacts = true;
        let entry = library_entry(&config, &status, &[]);
        assert_eq!(entry["artifactsEnabled"], true);
        assert_eq!(entry["artifactsDesignEnabled"], true);
        assert_eq!(entry["artifactsSlidesEnabled"], true);
        config.cowork_mode = "off".into();
        assert_eq!(library_entry(&config, &status, &[])["desktopHome"], "no-vm");
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
        let (remove, set) = code_env(
            &status,
            &none,
            inherited.iter().map(|s| s.to_string()),
            &Default::default(),
        )
        .unwrap();
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
        // Консоль — с настоящим ID модели, не с псевдонимом Desktop.
        assert_eq!(get("ANTHROPIC_MODEL"), Some("claude-p-1"));
        // Основная не выбрана — все роли на первой модели.
        for role in ["SONNET", "OPUS", "HAIKU", "FABLE"] {
            assert_eq!(
                get(&format!("ANTHROPIC_DEFAULT_{role}_MODEL")),
                Some("claude-p-1")
            );
        }
        assert!(get("ANTHROPIC_API_KEY").is_none());
        // Окно неизвестно (ни `[1m]`, ни каталога) — 1M и без DISABLE_1M: заниженное окно с
        // тяжёлыми MCP сжимало сессию по кругу (видели вживую на 200K и 626 инструментах).
        assert_eq!(get("CLAUDE_CODE_MAX_CONTEXT_TOKENS"), Some("1000000"));
        assert!(get("CLAUDE_CODE_DISABLE_1M_CONTEXT").is_none());

        let empty = UiStatus {
            models: Vec::new(),
            ..status
        };
        assert!(code_env(&empty, &none, std::iter::empty(), &Default::default()).is_err());

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
        println!(
            "{}",
            launch_code(&config, &status, &Default::default()).unwrap()
        );
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
        // Code (`epitaxy`) переключается на Chat (ADR-020); остальное в preferences цело.
        assert_eq!(saved["preferences"]["sidebarMode"], "chat");
        assert_eq!(saved["coworkUserFilesPath"], "C:\\x");

        // Явный Chat и незнакомое значение не трогаются.
        for mode in ["chat", "projects"] {
            fs::write(
                &path,
                format!(r#"{{"deploymentMode":"3p","preferences":{{"sidebarMode":"{mode}"}}}}"#),
            )
            .unwrap();
            set_deployment_mode(&path).unwrap();
            let saved: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(saved["preferences"]["sidebarMode"], mode);
        }

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
        // Окно неизвестно — образец с 1M (Opus 4.8): заниженное окно сжимало бы по кругу.
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

    #[test]
    fn a_1m_model_gets_one_menu_row() {
        let with_1m = |model: &str, openai: bool| ModelInfo {
            supports_1m: true,
            ..info(model, None, openai)
        };
        let rows = inference_models(
            &[
                with_1m("glm-5.3[1m]", true),
                with_1m("claude-haiku-5-5[1m]", true),
                with_1m("deepseek-v4[1m]", false),
                with_1m("claude-opus-5-5[1m]", false),
                info("qwen3", None, true),
                with_1m("anthropic/Claude-Sonnet-5-5[1m]", false),
                info("deepseek-x", Some("max"), true),
                info("claude-sonnet-5-5", None, false),
            ],
            &RoleAliases::default(),
        );
        // У провайдера модель одна: только вариант 1M, без строки «200K».
        assert_eq!(rows[0]["prefer1m"], true);
        assert_eq!(
            rows[1]["prefer1m"], true,
            "Claude через OpenAI-путь — окно провайдера"
        );
        assert_eq!(rows[2]["prefer1m"], true, "не-Claude за Anthropic API");
        // Настоящая Claude у Anthropic тоже одной строкой: у Claude 5.x окно 1M своё и цена одна.
        assert_eq!(rows[3]["prefer1m"], true);
        // Окно неизвестно, уровней нет (нет образца с 1M) — всё равно 1M, одной строкой.
        assert_eq!(rows[4]["supports1m"], true);
        assert_eq!(rows[4]["prefer1m"], true);
        assert_eq!(
            rows[5]["prefer1m"], true,
            "Claude под чужим ID у агрегатора"
        );
        assert!(rows.iter().take(4).all(|r| r["supports1m"] == true));
        // С уровнями окно 1M даёт образец `behavesAs`; строка одна и без [1m].
        assert_eq!(rows[6]["supports1m"], false);
        assert_eq!(rows[6]["behavesAs"], EFFORT_PROFILE);
        // Настоящая Claude без [1m]: окно Desktop берёт по её ID (у Sonnet 5.5 — 1M), строка одна.
        assert_eq!(rows[7]["supports1m"], false);
        assert!(rows[7].get("prefer1m").is_none());
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
    fn the_critic_model_reaches_scripts_only_when_chosen_and_served() {
        let dir = std::env::temp_dir()
            .join("agents")
            .join(format!("ctp-critic-{}", uuid::Uuid::new_v4().simple()));
        fs::create_dir_all(&dir).unwrap();
        let mut config = crate::store::load_or_create(&dir).unwrap();
        config.providers = vec![crate::model::ProviderRow {
            id: "oc".into(),
            kind: "openai".into(),
            base_url: "https://opencode.ai/v1".into(),
            models: vec!["glm-5.3[1m]".into(), "qwen4-max".into()],
            ..Default::default()
        }];
        let served: Vec<ModelInfo> = crate::store::alias_rows(&config.providers[0])
            .into_iter()
            .map(|row| ModelInfo {
                alias: row.alias,
                ..info(ctp_core::alias::strip_1m(&row.model), None, true)
            })
            .collect();
        let status = status_with(served);
        assert!(critic_env(&config, &status).is_empty(), "не выбран");

        config.model_roles.critic = Some(crate::model::ModelRef {
            provider: "oc".into(),
            model: "qwen4-max".into(),
        });
        let env = critic_env(&config, &status);
        let names: Vec<&str> = env.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, CRITIC_ENV);
        assert_eq!(env[0].1, "http://127.0.0.1:17899");
        assert_eq!(env[1].1, "tok");
        assert_eq!(env[2].1, "qwen4-max", "ID в шлюзе один — сам ID");
        assert_eq!(
            env[3].1, "pgc",
            "путь выбран, не угадывается по входу в Claude"
        );

        let mut stopped = status.clone();
        stopped.running = false;
        assert!(critic_env(&config, &stopped).is_empty(), "шлюз стоит");
        // Модель выбрана, но шлюз её ещё не отдаёт (не перезапускали).
        assert!(critic_env(&config, &status_with(status.models[..1].to_vec())).is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn main_role_goes_to_cli_env_and_desktop_tier() {
        let mut first = info("first", None, true);
        first.supports_1m = true;
        let main = info("main", None, true);
        let status = status_with(vec![first.clone(), main.clone()]);
        let roles = RoleAliases {
            sonnet: Some(main.alias.clone()),
        };
        let (_, set) = code_env(&status, &roles, std::iter::empty(), &Default::default()).unwrap();
        let get = |k: &str| set.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        assert_eq!(get("ANTHROPIC_MODEL"), Some(main.model.clone()));
        // Все роли `/model` — основная под её именем, а не встроенные модели Anthropic.
        for role in ["SONNET", "OPUS", "HAIKU", "FABLE"] {
            let var = format!("ANTHROPIC_DEFAULT_{role}_MODEL");
            assert_eq!(get(&var), Some(main.model.clone()));
            assert_eq!(get(&format!("{var}_NAME")), Some("main · P".into()));
            assert!(get(&format!("{var}_DESCRIPTION")).is_some());
        }
        assert_eq!(get("ANTHROPIC_SMALL_FAST_MODEL"), Some(main.model.clone()));
        // Тот же ID у моделей двух провайдеров шлюз развёл бы неоднозначно: псевдоним.
        let mut twin = info("main", None, true);
        twin.alias = "claude-q-main".into();
        let shared = status_with(vec![main.clone(), twin]);
        let (_, set) = code_env(&shared, &roles, std::iter::empty(), &Default::default()).unwrap();
        assert!(set.contains(&("ANTHROPIC_MODEL".into(), main.alias.clone())));
        assert!(!set.iter().any(|(n, _)| n.contains("ADVISOR")));
        // Окно основной модели неизвестно — 1M; предела вывода нет — Claude Code берёт свой.
        assert_eq!(
            get("CLAUDE_CODE_MAX_CONTEXT_TOKENS"),
            Some("1000000".into())
        );
        assert!(get("CLAUDE_CODE_MAX_OUTPUT_TOKENS").is_none());

        // Пределы модели из каталога: окно в рамках 100K–1M, вывод не выше 128K.
        let limits = |context, output| crate::catalog::Hint {
            context: Some(context),
            output: Some(output),
            ..Default::default()
        };
        for (hint, window, output) in [
            (limits(131_072, 8_192), "131072", "8192"),
            (limits(32_768, 4_096), "100000", "4096"),
            (limits(2_000_000, 384_000), "1000000", "128000"),
        ] {
            let (_, set) = code_env(&status, &roles, std::iter::empty(), &hint).unwrap();
            let get = |k: &str| set.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
            assert_eq!(
                get("CLAUDE_CODE_MAX_CONTEXT_TOKENS").as_deref(),
                Some(window)
            );
            assert_eq!(
                get("CLAUDE_CODE_MAX_OUTPUT_TOKENS").as_deref(),
                Some(output)
            );
            // `[1m]` у «Default» снимается только при известном окне меньше 1M.
            assert_eq!(
                get("CLAUDE_CODE_DISABLE_1M_CONTEXT").is_some(),
                window != "1000000"
            );
        }

        // Модель роли не запущена (шлюз со старым набором) — роль как не выбранная.
        let stale = RoleAliases {
            sonnet: Some("claude-gone".into()),
        };
        let (_, set) = code_env(&status, &stale, std::iter::empty(), &Default::default()).unwrap();
        assert!(set.contains(&("ANTHROPIC_MODEL".into(), first.model.clone())));
        assert!(set.contains(&("ANTHROPIC_DEFAULT_OPUS_MODEL".into(), first.model.clone())));

        // Desktop: основная помечена в своей записи, остальные — без роли.
        let rows = inference_models(&status.models, &roles);
        assert!(rows[0].get("anthropicFamilyTier").is_none());
        assert_eq!(rows[1]["anthropicFamilyTier"], "sonnet");
        assert_eq!(rows[1]["isFamilyDefault"], true);
    }
}
