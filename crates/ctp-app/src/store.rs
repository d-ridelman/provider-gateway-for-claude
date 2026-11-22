use crate::model::{AliasRow, Config, ProviderRow, UiConfig, UiProvider};
use crate::tr;
use ctp_core::alias::{desktop_alias, strip_1m};
use ctp_core::config::{ProviderConfig, ProvidersFile};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

const SERVICE: &str = "claude-provider-gateway";
static KEY_SERVICE: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Имя службы в Credential Manager. Ключи лежат по ID провайдера, поэтому у своей
/// папки (`CLAUDE_GATEWAY_HOME`) служба своя: иначе изолированный запуск мог бы
/// удалить или подменить ключ основного профиля с тем же ID.
pub fn service_for(home: Option<&Path>) -> String {
    match home {
        None => SERVICE.to_string(),
        Some(home) => format!("{SERVICE}@{}", home_key(home)),
    }
}

/// Один вид пути для одной папки: относительный путь раскрыт, без `\\?\`,
/// обратные слеши, без завершающего слеша, в нижнем регистре.
fn home_key(home: &Path) -> String {
    let full = std::fs::canonicalize(home).unwrap_or_else(|_| home.to_path_buf());
    let text = full.display().to_string().replace('/', "\\");
    let text = if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{unc}")
    } else {
        text.strip_prefix(r"\\?\").unwrap_or(&text).to_string()
    };
    text.trim_end_matches('\\').to_lowercase()
}

/// Вызывается один раз при старте, до первого обращения к ключам.
pub fn use_key_service(name: String) {
    let _ = KEY_SERVICE.set(name);
}

pub fn load_or_create(dir: &Path) -> anyhow::Result<Config> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join("config.json");
    let mut config: Config = if path.exists() {
        let text = std::fs::read_to_string(&path)?;
        // Блокнот сохраняет UTF-8 с BOM — serde такой файл не читает.
        match serde_json::from_str(text.trim_start_matches('\u{feff}')) {
            Ok(config) => config,
            Err(exc) => {
                // Битый файл не затираем молча: рядом остаётся копия для ручного разбора.
                // Если копию сделать нельзя — не стартуем, чтобы не потерять провайдеров.
                let stamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs());
                let backup = dir.join(format!("config.broken-{stamp}.json"));
                std::fs::copy(&path, &backup)?;
                tracing::error!(error = %exc, backup = %backup.display(), "config.json is unreadable, starting empty");
                Config::default()
            }
        }
    } else {
        Config::default()
    };
    normalize(&mut config);
    save(dir, &config)?;
    Ok(config)
}

pub fn save(dir: &Path, config: &Config) -> anyhow::Result<()> {
    let path = dir.join("config.json");
    let text = serde_json::to_string_pretty(config)?;
    std::fs::write(path, text)?;
    Ok(())
}

pub fn normalize(config: &mut Config) {
    if config.port == 0 {
        config.port = crate::model::DEFAULT_PORT;
    }
    if config.token.is_empty() {
        config.token = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
    }
    for provider in &mut config.providers {
        provider.id = provider.id.trim().to_ascii_lowercase();
        provider.name = provider.name.trim().to_string();
        if provider.name.is_empty() {
            provider.name = provider.id.clone();
        }
        provider.kind = if provider.kind.eq_ignore_ascii_case("anthropic") {
            "anthropic".to_string()
        } else {
            "openai".to_string()
        };
        provider.base_url = provider.base_url.trim().trim_end_matches('/').to_string();
        provider.models = clean_models(&provider.models);
        // Ручная правка config.json («MAX», опечатка) не должна молча выключать уровни при
        // следующем сохранении: нормализуем, неизвестное — выключено.
        provider.max_effort = clean_effort(provider.max_effort.take(), "").unwrap_or(None);
        provider.default_effort = clean_effort(provider.default_effort.take(), "").unwrap_or(None);
    }
}

pub fn clean_row(row: ProviderRow) -> Result<ProviderRow, String> {
    let row = clean_endpoint(row)?;
    if row.models.is_empty() {
        return Err(tr!(
            "Добавьте хотя бы одну модель",
            "Add at least one model"
        ));
    }
    Ok(row)
}

/// Проверка всего, кроме списка моделей: его можно загрузить уже после.
pub fn clean_endpoint(mut row: ProviderRow) -> Result<ProviderRow, String> {
    row.id = row.id.trim().to_ascii_lowercase();
    if row.id.is_empty() {
        return Err(tr!(
            "ID провайдера не может быть пустым",
            "Provider ID cannot be empty"
        ));
    }
    if !row
        .id
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
    {
        return Err(tr!(
            "ID: только латиница, цифры и дефис",
            "ID: only Latin letters, digits and hyphens"
        ));
    }
    row.name = row.name.trim().to_string();
    if row.name.is_empty() {
        row.name = row.id.clone();
    }
    row.kind = if row.kind.eq_ignore_ascii_case("anthropic") {
        "anthropic".to_string()
    } else {
        "openai".to_string()
    };
    row.base_url = row.base_url.trim().trim_end_matches('/').to_string();
    if row.base_url.is_empty() {
        return Err(tr!(
            "Укажите baseUrl провайдера",
            "Enter the provider's baseUrl"
        ));
    }
    if !(row.base_url.starts_with("http://") || row.base_url.starts_with("https://")) {
        return Err(tr!(
            "baseUrl должен начинаться с http:// или https://",
            "baseUrl must start with http:// or https://"
        ));
    }
    row.models = clean_models(&row.models);
    row.max_effort = clean_effort(
        row.max_effort.take(),
        &tr!("Макс. уровень рассуждения", "Max reasoning level"),
    )?;
    row.default_effort = clean_effort(
        row.default_effort.take(),
        &tr!("Уровень по умолчанию", "Default level"),
    )?;
    // Имена — только для моделей из списка, без пустых.
    let ids: HashSet<String> = row
        .models
        .iter()
        .map(|m| strip_1m(m).trim().to_string())
        .collect();
    row.labels = std::mem::take(&mut row.labels)
        .into_iter()
        .map(|(model, label)| (model.trim().to_string(), label.trim().to_string()))
        .filter(|(model, label)| !label.is_empty() && ids.contains(model))
        .collect();
    Ok(row)
}

/// `None`, пусто и `off` — выключено; иначе один из уровней Claude.
fn clean_effort(value: Option<String>, label: &str) -> Result<Option<String>, String> {
    let Some(value) = value.map(|v| v.trim().to_ascii_lowercase()) else {
        return Ok(None);
    };
    if value.is_empty() || value == "off" {
        return Ok(None);
    }
    if ctp_gateway::state::EFFORT_LEVELS.contains(&value.as_str()) {
        Ok(Some(value))
    } else {
        Err(tr!(
            "{label}: неизвестное значение «{value}»",
            "{label}: unknown value \"{value}\""
        ))
    }
}

/// Новый конфиг с добавленным или заменённым провайдером; `original` — ID до правки
/// (`None` для нового). Позиция в списке сохраняется: от неё зависит модель по умолчанию.
pub fn apply_provider(
    config: &Config,
    row: ProviderRow,
    original: Option<&str>,
) -> Result<Config, String> {
    let clash = config
        .providers
        .iter()
        .any(|p| p.id == row.id && original.is_none_or(|o| p.id != o));
    if clash {
        let id = &row.id;
        return Err(tr!(
            "Провайдер с ID «{id}» уже есть",
            "A provider with ID \"{id}\" already exists"
        ));
    }
    let mut next = config.clone();
    // Переименованный провайдер уносит с собой выбранные на его модели роли.
    if let Some(old) = original.filter(|old| *old != row.id) {
        let roles = &mut next.model_roles;
        for role in [&mut roles.sonnet, &mut roles.opus, &mut roles.haiku]
            .into_iter()
            .flatten()
            .filter(|role| role.provider == old)
        {
            role.provider = row.id.clone();
        }
    }
    let slot = original.and_then(|o| next.providers.iter_mut().find(|p| p.id == o));
    match slot {
        Some(existing) => *existing = row,
        None => next.providers.push(row),
    }
    Ok(next)
}

fn clean_models(models: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    models
        .iter()
        .map(|model| model.trim().to_string())
        .filter(|model| !model.is_empty() && seen.insert(model.clone()))
        .collect()
}

pub fn alias_rows(row: &ProviderRow) -> Vec<AliasRow> {
    row.models
        .iter()
        .filter_map(|model| {
            let id = strip_1m(model).trim();
            if id.is_empty() {
                return None;
            }
            Some(AliasRow {
                model: model.clone(),
                alias: desktop_alias(&row.id, id),
                supports_1m: model.to_ascii_lowercase().ends_with("[1m]"),
                label: row.labels.get(id).cloned(),
            })
        })
        .collect()
}

pub fn ui_config(config: &Config, dir: &Path) -> UiConfig {
    let providers = config
        .providers
        .iter()
        .map(|row| UiProvider {
            id: row.id.clone(),
            name: row.name.clone(),
            kind: row.kind.clone(),
            base_url: row.base_url.clone(),
            models: row.models.clone(),
            has_key: get_key(&row.id).is_some(),
            needs_key: !is_local(&row.base_url),
            aliases: alias_rows(row),
            max_effort: row.max_effort.clone(),
            default_effort: row.default_effort.clone(),
            labels: row.labels.clone(),
        })
        .collect();
    let settings = crate::cc_settings::settings_path();
    let advisor = match &settings {
        Some(path) => crate::cc_settings::read_advisor(path),
        None => Err(tr!("не найдена домашняя папка", "home folder not found")),
    };
    UiConfig {
        port: config.port,
        token: config.token.clone(),
        claude_path: config.claude_path.clone(),
        detected_claude: crate::claude::detect(config).map(|path| path.display().to_string()),
        providers,
        russian_ui: config.russian_ui,
        builtin_browser: config.builtin_browser,
        code_dir: config.code_dir.clone(),
        language: crate::i18n::current().to_string(),
        language_pref: config
            .language
            .clone()
            .unwrap_or_else(|| "auto".to_string()),
        theme_pref: config.theme.clone().unwrap_or_else(|| "auto".to_string()),
        auto_update: config.auto_update,
        version: env!("CARGO_PKG_VERSION").to_string(),
        russian_file: dir.join(crate::ruui::TRANSLATION_FILE).is_file(),
        config_dir: dir.display().to_string(),
        model_roles: config.model_roles.clone(),
        advisor_model: advisor.as_ref().ok().cloned().flatten(),
        advisor_decided: config.advisor_decided,
        claude_settings: settings
            .map(|path| path.display().to_string())
            .unwrap_or_default(),
        claude_settings_error: advisor.err(),
        advisor_notice: None,
    }
}

/// Псевдоним шлюза для модели роли; провайдера или модели больше нет — `None`.
pub fn role_alias(
    config: &crate::model::Config,
    role: Option<&crate::model::ModelRef>,
) -> Option<String> {
    let role = role?;
    let provider = config.providers.iter().find(|p| p.id == role.provider)?;
    // Без `[1m]`: дописанный или убранный суффикс не сбрасывает выбор.
    let wanted = strip_1m(&role.model).trim();
    alias_rows(provider)
        .into_iter()
        .find(|row| strip_1m(&row.model).trim() == wanted)
        .map(|row| row.alias)
}

/// Провайдер в форме, понятной шлюзу: псевдоним → ID модели (без `[1m]`).
pub fn provider_config(row: &ProviderRow, api_key: Option<String>) -> ProviderConfig {
    ProviderConfig {
        id: row.id.clone(),
        name: Some(row.name.clone()),
        kind: Some(row.kind.clone()),
        base_url: row.base_url.clone(),
        api_key: api_key.filter(|key| !key.is_empty()),
        models: alias_rows(row)
            .into_iter()
            .map(|alias| (alias.alias, strip_1m(&alias.model).trim().to_string()))
            .collect(),
        headers: BTreeMap::new(),
        chat_completions_url: None,
        max_tokens_field: None,
        extra_body: None,
        include_usage: None,
        max_effort: row.max_effort.clone(),
        default_effort: row.default_effort.clone(),
        // Для opencode.ai шлюз подставляет заголовок сам.
        session_header: None,
    }
}

pub fn build_file(config: &Config) -> Result<ProvidersFile, String> {
    let providers: Vec<ProviderConfig> = config
        .providers
        .iter()
        .map(|row| provider_config(row, get_key(&row.id)))
        .filter(|provider| !provider.models.is_empty())
        .collect();
    if providers.is_empty() {
        return Err(tr!(
            "Нет ни одного провайдера с моделями",
            "No providers with models"
        ));
    }
    // Модель для запросов с незнакомым именем — основная (роль sonnet), иначе первая.
    // Так невыбранные роли Claude Code (его встроенные ID opus/haiku) и в консоли, и во
    // вкладке Code у Desktop попадают на основную модель.
    let default = role_alias(config, config.model_roles.sonnet.as_ref()).or_else(|| {
        providers
            .first()
            .and_then(|provider| provider.models.first().map(|(alias, _)| alias.clone()))
    });
    // Советник (advisor Claude Code) — сильная модель; не выбрана — советника нет.
    let advisor = role_alias(config, config.model_roles.opus.as_ref());
    Ok(ProvidersFile {
        default,
        advisor,
        providers,
    })
}

fn is_local(base_url: &str) -> bool {
    base_url.contains("127.0.0.1") || base_url.contains("localhost") || base_url.contains("[::1]")
}

fn entry(id: &str) -> Result<keyring::Entry, String> {
    let service = KEY_SERVICE.get().map_or(SERVICE, String::as_str);
    keyring::Entry::new(service, id).map_err(|exc| exc.to_string())
}

pub fn get_key(id: &str) -> Option<String> {
    read_key(id).ok().flatten()
}

/// `Ok(None)` — ключа нет; `Err` — Credential Manager не ответил (не путать с «нет ключа»).
pub fn read_key(id: &str) -> Result<Option<String>, String> {
    match entry(id)?.get_password() {
        Ok(key) => Ok(Some(key)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(exc) => Err(format!("Credential Manager: {exc}")),
    }
}

pub fn set_key(id: &str, key: &str) -> Result<(), String> {
    entry(id)?.set_password(key).map_err(|exc| {
        tr!(
            "Не удалось сохранить ключ в Windows Credential Manager: {exc}",
            "Could not save the key to Windows Credential Manager: {exc}"
        )
    })
}

pub fn delete_key(id: &str) {
    if let Ok(entry) = entry(id) {
        let _ = entry.delete_credential();
    }
}

const LOG_LIMIT: u64 = 5 * 1024 * 1024;
const TAIL_BYTES: u64 = 256 * 1024;

/// При старте: журнал больше 5 МБ уезжает в `gateway.log.1` (прошлый `.1` затирается).
pub fn rotate_log(dir: &Path) {
    let path = dir.join("gateway.log");
    if std::fs::metadata(&path).is_ok_and(|meta| meta.len() > LOG_LIMIT) {
        let _ = std::fs::rename(&path, dir.join("gateway.log.1"));
    }
}

/// Последние `lines` строк журнала; читается только хвост файла.
pub fn tail_log(dir: &Path, lines: usize) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut file) = std::fs::File::open(dir.join("gateway.log")) else {
        return tr!("Логов пока нет", "No logs yet");
    };
    let len = file.metadata().map(|meta| meta.len()).unwrap_or(0);
    let start = len.saturating_sub(TAIL_BYTES);
    let mut bytes = Vec::new();
    if file.seek(SeekFrom::Start(start)).is_err() || file.read_to_end(&mut bytes).is_err() {
        return tr!("Журнал не читается", "Log is unreadable");
    }
    let text = String::from_utf8_lossy(&bytes);
    // С середины файла первая строка обрезана — её не показываем.
    let text = if start > 0 {
        text.split_once('\n').map_or("", |(_, rest)| rest)
    } else {
        &text
    };
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> ProviderRow {
        ProviderRow {
            id: "openai".into(),
            name: "OpenAI".into(),
            kind: "openai".into(),
            base_url: "https://api.openai.com/v1".into(),
            models: vec!["gpt-5.2".into(), "qwen3:8b[1m]".into()],
            ..ProviderRow::default()
        }
    }

    #[test]
    fn labels_are_kept_only_for_listed_models_and_reach_aliases() {
        let mut r = row();
        r.labels = [
            ("qwen3:8b".to_string(), " Qwen 3 ".to_string()),
            ("gone".to_string(), "Gone".to_string()),
            ("gpt-5.2".to_string(), "  ".to_string()),
        ]
        .into();
        let cleaned = clean_row(r).unwrap();
        assert_eq!(cleaned.labels.len(), 1);
        let rows = alias_rows(&cleaned);
        assert_eq!(rows[1].label.as_deref(), Some("Qwen 3"), "ID без [1m]");
        assert_eq!(rows[0].label, None);
        // Пустые имена не пишутся в config.json вовсе.
        let json = serde_json::to_value(row()).unwrap();
        assert!(json.get("labels").is_none());
    }

    #[test]
    fn effort_levels_are_validated_and_passed_to_gateway() {
        let mut r = row();
        r.max_effort = Some(" MAX ".into());
        r.default_effort = Some("off".into());
        let cleaned = clean_row(r).unwrap();
        assert_eq!(cleaned.max_effort.as_deref(), Some("max"));
        assert_eq!(cleaned.default_effort, None);
        let cfg = provider_config(&cleaned, None);
        assert_eq!(cfg.max_effort.as_deref(), Some("max"));

        let mut bad = row();
        bad.max_effort = Some("ultra".into());
        assert!(clean_row(bad).is_err());
    }

    #[test]
    fn aliases_and_one_million_flag() {
        let aliases = alias_rows(&row());
        assert_eq!(aliases.len(), 2);
        // ID `openai` Claude Desktop отбрасывает — нейтральная метка вместо него.
        assert!(
            aliases[0].alias.starts_with("claude-p"),
            "{}",
            aliases[0].alias
        );
        assert!(!ctp_core::alias::desktop_rejects(&aliases[0].alias));
        assert!(!aliases[0].supports_1m);
        assert!(aliases[1].supports_1m);
        assert_eq!(aliases[1].alias, desktop_alias("openai", "qwen3:8b"));
    }

    #[test]
    fn builds_a_providers_file_without_keys() {
        let config = Config {
            port: 17899,
            token: "local-token".into(),
            providers: vec![row()],
            ..Config::default()
        };
        let file = build_file(&config).unwrap();
        assert_eq!(file.providers.len(), 1);
        assert_eq!(file.providers[0].models.len(), 2);
        assert_eq!(file.providers[0].models[1].1, "qwen3:8b");
        assert_eq!(
            file.default.as_deref(),
            Some(desktop_alias("openai", "gpt-5.2").as_str())
        );
        // Выбрана основная модель — она и модель по умолчанию для незнакомых имён.
        let mut with_main = config.clone();
        with_main.model_roles.sonnet = Some(crate::model::ModelRef {
            provider: "openai".into(),
            model: "qwen3:8b".into(),
        });
        assert_eq!(
            build_file(&with_main).unwrap().default,
            Some(desktop_alias("openai", "qwen3:8b"))
        );
    }

    #[test]
    fn rejects_bad_provider_id() {
        let mut bad = row();
        bad.id = "OpenAI API".into();
        assert!(clean_row(bad).is_err());
    }

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir()
            .join("agents")
            .join(format!("ctp-app-{tag}-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn broken_config_is_backed_up_not_lost() {
        let dir = temp_dir("broken");
        std::fs::write(dir.join("config.json"), "{ not json").unwrap();
        let config = load_or_create(&dir).unwrap();
        assert!(config.providers.is_empty());
        let backups: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("config.broken-")
            })
            .collect();
        assert_eq!(backups.len(), 1);
        assert_eq!(
            std::fs::read_to_string(backups[0].path()).unwrap(),
            "{ not json"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Ключ должен пережить новый `Entry` (то есть перезапуск приложения).
    /// Mock-хранилище keyring этого не умеет — тест ловит пропавшую фичу `windows-native`.
    #[test]
    fn key_survives_a_fresh_entry() {
        let id = format!("ctp-test-{}", uuid::Uuid::new_v4().simple());
        set_key(&id, "sk-test-persist").unwrap();
        let read = get_key(&id);
        delete_key(&id);
        assert_eq!(read.as_deref(), Some("sk-test-persist"));
        assert_eq!(read_key(&id), Ok(None));
    }

    #[test]
    fn theme_is_auto_until_it_is_chosen() {
        let dir = temp_dir("theme");
        let old: Config = serde_json::from_str(r#"{"port":17899}"#).unwrap();
        assert_eq!(ui_config(&old, &dir).theme_pref, "auto");
        let dark: Config = serde_json::from_str(r#"{"theme":"dark"}"#).unwrap();
        assert_eq!(ui_config(&dark, &dir).theme_pref, "dark");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn own_home_gets_own_key_service() {
        assert_eq!(service_for(None), "claude-provider-gateway");
        // Несуществующая папка: canonicalize не сработает, остаётся текстовая нормализация.
        let own = service_for(Some(Path::new(r"Q:\Portable\GW")));
        assert_ne!(own, service_for(None));
        assert_eq!(own, r"claude-provider-gateway@q:\portable\gw");
        for variant in [
            r"q:\PORTABLE\gw",
            r"Q:\Portable\GW\",
            "Q:/Portable/GW",
            r"\\?\Q:\Portable\GW",
        ] {
            assert_eq!(service_for(Some(Path::new(variant))), own, "{variant}");
        }

        // Существующая папка: canonicalize раскрывает `..`, без `\\?\`; слеш в конце не важен.
        // (Рабочую папку процесса не меняем: тесты идут параллельно.)
        let dir = temp_dir("home");
        let name = service_for(Some(&dir));
        assert!(!name.contains(r"\\?\"), "{name}");
        let dotted = dir.join("..").join(dir.file_name().unwrap());
        assert_eq!(service_for(Some(&dotted)), name);
        assert_eq!(service_for(Some(&dir.join(""))), name);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn config_with_bom_is_read() {
        let dir = temp_dir("bom");
        std::fs::write(
            dir.join("config.json"),
            "\u{feff}{\"port\":18001,\"token\":\"t\",\"providers\":[{\"id\":\"x\",\"baseUrl\":\"http://h\",\"models\":[\"m\"]}]}",
        )
        .unwrap();
        let config = load_or_create(&dir).unwrap();
        assert_eq!(config.port, 18001);
        assert_eq!(config.providers.len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn apply_provider_adds_renames_and_rejects_clashes() {
        let base = Config {
            providers: vec![row()],
            ..Config::default()
        };
        let mut other = row();
        other.id = "deepseek".into();

        let added = apply_provider(&base, other.clone(), None).unwrap();
        assert_eq!(added.providers.len(), 2);
        assert!(apply_provider(&added, other.clone(), None).is_err());

        let mut renamed = row();
        renamed.id = "openai2".into();
        let mut with_role = added.clone();
        with_role.model_roles.opus = Some(crate::model::ModelRef {
            provider: "openai".into(),
            model: "gpt-5.2".into(),
        });
        let next = apply_provider(&with_role, renamed, Some("openai")).unwrap();
        assert_eq!(next.providers[0].id, "openai2", "позиция сохраняется");
        assert_eq!(next.providers.len(), 2);
        assert_eq!(
            next.model_roles.opus.as_ref().map(|r| r.provider.as_str()),
            Some("openai2"),
            "роль переезжает вместе с провайдером"
        );

        let mut onto = row();
        onto.id = "deepseek".into();
        assert!(apply_provider(&added, onto, Some("openai")).is_err());
        assert!(apply_provider(&added, row(), Some("openai")).is_ok());
    }

    #[test]
    fn tail_reads_last_lines_and_rotation_moves_big_log() {
        let dir = temp_dir("log");
        assert_eq!(tail_log(&dir, 5), "Логов пока нет");
        let body: String = (0..30_000).map(|i| format!("line {i}\n")).collect();
        std::fs::write(dir.join("gateway.log"), &body).unwrap();
        assert_eq!(tail_log(&dir, 2), "line 29998\nline 29999");

        rotate_log(&dir);
        assert!(
            dir.join("gateway.log").exists(),
            "маленький журнал не трогаем"
        );
        std::fs::write(dir.join("gateway.log"), vec![b'x'; LOG_LIMIT as usize + 1]).unwrap();
        rotate_log(&dir);
        assert!(!dir.join("gateway.log").exists());
        assert!(dir.join("gateway.log.1").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn deduplicates_models() {
        let mut row = row();
        row.models = vec!["gpt-5.2".into(), "gpt-5.2".into(), " ".into()];
        row.id = "p".into();
        let cleaned = clean_row(row).unwrap();
        assert_eq!(cleaned.models, vec!["gpt-5.2"]);
    }

    #[test]
    fn role_points_to_a_model_by_provider_and_id() {
        use crate::model::{Config, ModelRef};
        let mut provider = row();
        provider.id = "oc".into();
        provider.models = vec!["glm-5.3[1m]".into(), "deepseek-v4".into()];
        let config = Config {
            providers: vec![provider.clone()],
            ..Default::default()
        };
        let glm = alias_rows(&provider)[0].alias.clone();
        let role = |provider: &str, model: &str| ModelRef {
            provider: provider.into(),
            model: model.into(),
        };
        // `[1m]` в выборе и в списке может различаться — та же модель.
        assert_eq!(
            role_alias(&config, Some(&role("oc", "glm-5.3"))),
            Some(glm.clone())
        );
        assert_eq!(
            role_alias(&config, Some(&role("oc", "glm-5.3[1m]"))),
            Some(glm)
        );
        assert_eq!(role_alias(&config, Some(&role("oc", "gone"))), None);
        assert_eq!(role_alias(&config, Some(&role("other", "glm-5.3"))), None);
        assert_eq!(role_alias(&config, None), None);
    }
}
