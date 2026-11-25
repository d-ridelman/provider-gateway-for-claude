//! Модель шлюза не остаётся в общих настройках Claude Code (ADR-010): `/model` с Enter в
//! консоли PGC пишет её в `settings.json` человека, и обычный `claude` без PGC открывался
//! бы на модели, которой у Anthropic нет.

use crate::model::Config;
use ctp_core::alias::strip_1m;
use serde_json::{Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

/// Своя модель человека до консоли PGC (`null` — ключа не было).
const SNAPSHOT: &str = "code-model-before.json";

/// Где запомнена модель: одно место для всех сборок PGC (release и dev), ведь `settings.json`
/// у них общий. Изолированный запуск (`CLAUDE_GATEWAY_HOME`) настоящие настройки не трогает.
fn snapshot_path() -> Option<PathBuf> {
    if std::env::var_os("CLAUDE_GATEWAY_HOME").is_some_and(|home| !home.is_empty()) {
        return None;
    }
    Some(
        crate::claude::local_appdata()?
            .join("ClaudeProviderGateway")
            .join(SNAPSHOT),
    )
}

/// Настройки Claude Code как объект и был ли перевод строки в конце файла.
type Settings = (Map<String, Value>, bool);

/// Общие настройки, которые читает установленный `claude`, запущенный из PGC
/// (`CLAUDE_CONFIG_DIR` консоль наследует).
fn user_settings() -> Option<PathBuf> {
    let dir = std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| Some(PathBuf::from(std::env::var_os("USERPROFILE")?).join(".claude")))?;
    Some(dir.join("settings.json"))
}

/// Модель шлюза: псевдоним PGC или ID модели провайдера. ID `claude-…` — не шлюза: они
/// работают у Anthropic напрямую. Псевдонимы Claude Code (`opus`) не модели провайдера.
pub fn is_gateway_model(config: &Config, model: &str) -> bool {
    let model = strip_1m(model.trim()).trim().to_ascii_lowercase();
    if model.is_empty() {
        return false;
    }
    config
        .providers
        .iter()
        .flat_map(crate::store::alias_rows)
        .any(|row| {
            row.alias.eq_ignore_ascii_case(&model)
                || (!model.starts_with("claude-")
                    && strip_1m(&row.model).trim().eq_ignore_ascii_case(&model))
        })
}

/// Настройки как объект и признак перевода строки в конце; `Ok(None)` — файла нет.
/// Файл, который не читается или не объект, — `Err`: такой не трогаем.
fn read_settings(path: &Path) -> Result<Option<Settings>, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(exc) if exc.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(exc) => return Err(format!("{}: {exc}", path.display())),
    };
    match serde_json::from_str(text.trim_start_matches('\u{feff}')) {
        Ok(Value::Object(map)) => Ok(Some((map, text.ends_with('\n')))),
        _ => Err(format!("{}: not a JSON object", path.display())),
    }
}

fn leaked(config: &Config, value: Option<&Value>) -> bool {
    value
        .and_then(Value::as_str)
        .is_some_and(|model| is_gateway_model(config, model))
}

/// Перед консолью: запомнить свою модель человека (последнюю). Модель шлюза не
/// запоминается и не затирает запомненное раньше; нет его — при возврате ключ уберётся.
fn remember_at(settings: &Path, snapshot: &Path, config: &Config) -> Result<(), String> {
    let current = read_settings(settings)?
        .and_then(|(mut map, _)| map.remove("model"))
        .unwrap_or(Value::Null);
    if leaked(config, Some(&current)) {
        return Ok(());
    }
    crate::store::write_file(snapshot, &current.to_string())
}

/// При старте и выходе PGC: модель шлюза в настройках — назад к запомненной (или без ключа).
/// `Ok(true)` — файл исправлен, запомненное использовано и стёрто. Иначе оно остаётся: консоль
/// PGC может пережить сам PGC и выбрать модель шлюза уже после его выхода.
fn restore_at(settings: &Path, snapshot: &Path, config: &Config) -> Result<bool, String> {
    let before = fs::read_to_string(snapshot)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .filter(|value| !value.is_null() && !leaked(config, Some(value)));
    let fixed = (|| {
        let Some((mut map, newline)) = read_settings(settings)? else {
            return Ok(false);
        };
        // The person sends plain `claude` to their own endpoint from this file: a model that
        // happens to share a gateway model's ID is theirs. The PGC console never sets it here.
        let own_endpoint = map
            .get("env")
            .is_some_and(|env| env.get("ANTHROPIC_BASE_URL").is_some());
        if own_endpoint || !leaked(config, map.get("model")) {
            return Ok(false);
        }
        match before {
            // insert на месте старого ключа: порядок остальных не меняется.
            Some(value) => {
                map.insert("model".into(), value);
            }
            None => {
                map.shift_remove("model");
            }
        }
        let mut text =
            serde_json::to_string_pretty(&Value::Object(map)).map_err(|exc| exc.to_string())?;
        if newline {
            text.push('\n');
        }
        crate::store::write_file(settings, &text)?;
        Ok(true)
    })();
    if fixed == Ok(true) {
        let _ = fs::remove_file(snapshot);
    }
    fixed
}

/// Перед запуском консоли PGC (не портативной — у той своя папка настроек).
pub fn remember(config: &Config) {
    if config.code_portable {
        return;
    }
    if let (Some(settings), Some(snapshot)) = (user_settings(), snapshot_path()) {
        let remembered = snapshot
            .parent()
            .map_or(Ok(()), fs::create_dir_all)
            .map_err(|exc| exc.to_string())
            .and_then(|()| remember_at(&settings, &snapshot, config));
        if let Err(exc) = remembered {
            tracing::warn!(error = %exc, "Claude Code model not remembered");
        }
    }
}

/// При старте и выходе PGC.
pub fn restore(config: &Config) {
    let (Some(settings), Some(snapshot)) = (user_settings(), snapshot_path()) else {
        return;
    };
    match restore_at(&settings, &snapshot, config) {
        Ok(true) => tracing::info!("gateway model removed from Claude Code settings"),
        Ok(false) => {}
        Err(exc) => tracing::warn!(error = %exc, "Claude Code settings not checked"),
    }
}

/// При выходе PGC (трей, окно, конец сеанса Windows).
pub fn restore_on_exit(app: &tauri::AppHandle) {
    use tauri::Manager;
    if let Some(shared) = app.try_state::<crate::state::AppShared>() {
        let config = shared
            .config
            .lock()
            .unwrap_or_else(|exc| exc.into_inner())
            .clone();
        restore(&config);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ProviderRow;

    /// Своя папка: настройки Claude Code, запомненное и конфиг PGC с провайдером OpenCode.
    fn setup() -> (PathBuf, PathBuf, PathBuf, Config) {
        let dir = std::env::temp_dir()
            .join("agents")
            .join(format!("ctp-codemodel-{}", uuid::Uuid::new_v4().simple()));
        fs::create_dir_all(&dir).unwrap();
        let mut config = crate::store::load_or_create(&dir).unwrap();
        config.providers = vec![ProviderRow {
            id: "opencode".into(),
            kind: "openai".into(),
            base_url: "https://opencode.ai/v1".into(),
            models: vec!["glm-5.3[1m]".into(), "claude-haiku-5-5[1m]".into()],
            ..Default::default()
        }];
        (
            dir.join("settings.json"),
            dir.join(SNAPSHOT),
            dir.clone(),
            config,
        )
    }

    #[test]
    fn gateway_models_are_told_from_models_that_work_without_it() {
        let (_, _, dir, config) = setup();
        let alias = crate::store::alias_rows(&config.providers[0])[0]
            .alias
            .clone();
        assert!(is_gateway_model(&config, "glm-5.3"));
        assert!(is_gateway_model(&config, "GLM-5.3[1m]"));
        assert!(is_gateway_model(&config, &alias));
        assert!(is_gateway_model(&config, &format!("{alias}[1m]")));
        // У Anthropic есть сами: PGC их не трогает, даже если провайдер их отдаёт.
        assert!(!is_gateway_model(&config, "claude-haiku-5-5"));
        assert!(!is_gateway_model(&config, "opus"));
        assert!(!is_gateway_model(&config, "sonnet[1m]"));
        assert!(!is_gateway_model(&config, "deepseek-v4"));
        assert!(!is_gateway_model(&config, ""));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_gateway_model_goes_back_to_the_one_before_the_console() {
        let (settings, snapshot, dir, config) = setup();
        fs::write(
            &settings,
            "{\n  \"env\": {},\n  \"model\": \"opus\",\n  \"theme\": \"dark\"\n}\n",
        )
        .unwrap();
        remember_at(&settings, &snapshot, &config).unwrap();
        // Между консолями человек сам сменил модель — вернуть надо последнюю.
        fs::write(
            &settings,
            "{\n  \"env\": {},\n  \"model\": \"sonnet\",\n  \"theme\": \"dark\"\n}\n",
        )
        .unwrap();
        remember_at(&settings, &snapshot, &config).unwrap();
        // В консоли PGC выбрали модель шлюза через `/model`.
        fs::write(
            &settings,
            "{\n  \"env\": {},\n  \"model\": \"glm-5.3\",\n  \"theme\": \"dark\"\n}\n",
        )
        .unwrap();
        // Вторая консоль не затирает запомненное моделью шлюза.
        remember_at(&settings, &snapshot, &config).unwrap();
        assert!(restore_at(&settings, &snapshot, &config).unwrap());
        assert_eq!(
            fs::read_to_string(&settings).unwrap(),
            "{\n  \"env\": {},\n  \"model\": \"sonnet\",\n  \"theme\": \"dark\"\n}\n",
            "последняя своя модель на своём месте, остальное как было"
        );
        assert!(!snapshot.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_gateway_model_without_a_remembered_one_is_removed() {
        let (settings, snapshot, dir, config) = setup();
        // Модель шлюза была ещё до консоли (как glm-5.3 с 24.11): запоминать нечего.
        fs::write(&settings, r#"{"model":"glm-5.3[1m]","theme":"dark"}"#).unwrap();
        remember_at(&settings, &snapshot, &config).unwrap();
        assert!(!snapshot.exists());
        assert!(restore_at(&settings, &snapshot, &config).unwrap());
        let left: Value = serde_json::from_str(&fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!(left, serde_json::json!({"theme": "dark"}));
        // До консоли ключа не было: запомнено «нет», и вернуть надо «нет», а не `null`.
        remember_at(&settings, &snapshot, &config).unwrap();
        assert_eq!(fs::read_to_string(&snapshot).unwrap(), "null");
        fs::write(&settings, r#"{"theme":"dark","model":"glm-5.3"}"#).unwrap();
        assert!(restore_at(&settings, &snapshot, &config).unwrap());
        let left: Value = serde_json::from_str(&fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!(left, serde_json::json!({"theme": "dark"}));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn own_choices_and_broken_files_are_left_alone() {
        let (settings, snapshot, dir, config) = setup();
        for text in [
            r#"{"model":"claude-haiku-5-5","theme":"dark"}"#,
            r#"{"model":"opus"}"#,
            r#"{"theme":"dark"}"#,
            "{ broken",
            r#"["glm-5.3"]"#,
            // Their own endpoint in this file: the same ID there is their model, not a leak.
            r#"{"model":"glm-5.3","env":{"ANTHROPIC_BASE_URL":"http://127.0.0.1:4000"}}"#,
        ] {
            fs::write(&settings, text).unwrap();
            fs::write(&snapshot, r#""sonnet""#).unwrap();
            let _ = restore_at(&settings, &snapshot, &config);
            assert_eq!(fs::read_to_string(&settings).unwrap(), text);
            // Не пригодилось — остаётся: открытая консоль ещё может выбрать модель шлюза.
            assert!(snapshot.exists(), "{text}");
        }
        // Нет файла — нечего исправлять, и он не создаётся.
        fs::remove_file(&settings).unwrap();
        assert!(!restore_at(&settings, &snapshot, &config).unwrap());
        assert!(!settings.exists());
        fs::remove_dir_all(dir).unwrap();
    }
}
