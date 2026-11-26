//! Окно сжатия разговора (ADR-019): одно поле PGC для Desktop и консоли.
//!
//! Консоль получает `CLAUDE_CODE_AUTO_COMPACT_WINDOW`. Сессии Desktop читают
//! `modelSettings` общего `settings.json`; Claude Code 2.1.330 для модели с `[1m]` читает
//! ключ с `[1m]`, а `/autocompact` пишет без него, поэтому PGC пишет оба ключа.

use crate::model::Config;
use serde_json::{Map, Value};
use std::path::Path;

/// Меньше Claude Code окно сжатия не принимает.
pub const MIN: u64 = 100_000;
pub const MAX: u64 = 1_000_000;
/// Окно обычной строки модели в Desktop; со строкой `[1m]` — `MAX` (ADR-011).
const PLAIN_WINDOW: u64 = 200_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Spec {
    Tokens(u64),
    Percent(u64),
}

/// `400k`, `0.5m`, `400000` или `40%`; токены от `MIN`, процент 1–100. Иначе `None`.
pub fn parse(text: &str) -> Option<Spec> {
    let text = text.trim().to_ascii_lowercase();
    if let Some(percent) = text.strip_suffix('%') {
        let percent: u64 = percent.trim().parse().ok()?;
        return (1..=100)
            .contains(&percent)
            .then_some(Spec::Percent(percent));
    }
    let (number, scale) = match text.chars().last()? {
        'k' => (&text[..text.len() - 1], 1_000.0),
        'm' => (&text[..text.len() - 1], 1_000_000.0),
        _ => (text.as_str(), 1.0),
    };
    let value: f64 = number.trim().parse().ok()?;
    let tokens = (value * scale).round();
    (tokens.is_finite() && tokens >= MIN as f64).then(|| Spec::Tokens((tokens as u64).min(MAX)))
}

/// Окно сжатия в токенах для модели с окном `window`.
pub fn tokens(spec: Spec, window: u64) -> u64 {
    match spec {
        Spec::Tokens(tokens) => tokens,
        Spec::Percent(percent) => window * percent / 100,
    }
    .clamp(MIN, MAX)
}

/// Значение для консоли PGC; `window` — окно, которое PGC задаёт этой консоли.
pub fn console_tokens(config: &Config, window: u64) -> Option<u64> {
    let spec = parse(config.compact_window.as_deref()?)?;
    Some(tokens(spec, window))
}

/// Ключи `modelSettings`, которые PGC ведёт: псевдоним и псевдоним с `[1m]` каждой модели
/// шлюза, с окном той строки меню, которой они соответствуют.
fn model_keys(config: &Config) -> Vec<(String, u64)> {
    config
        .providers
        .iter()
        .flat_map(crate::store::alias_rows)
        .flat_map(|row| {
            [
                (format!("{}[1m]", row.alias), MAX),
                (row.alias, PLAIN_WINDOW),
            ]
        })
        .collect()
}

/// Привести `modelSettings` в соответствие с полем. `prev` — прежнее значение поля: убираем
/// только то, что PGC записал бы по нему, чужие значения остаются. `Ok(true)` — файл записан.
pub fn sync_at(path: &Path, config: &Config, prev: Option<&str>) -> Result<bool, String> {
    let next = config.compact_window.as_deref().and_then(parse);
    let prev = prev.and_then(parse);
    if next.is_none() && prev.is_none() {
        return Ok(false);
    }
    let (mut map, newline) = match crate::codemodel::read_settings(path)? {
        Some(settings) => settings,
        None if next.is_some() => (Map::new(), true),
        None => return Ok(false),
    };
    let before = map.clone();
    let keys = model_keys(config);
    let mut models = match map.get("modelSettings") {
        None => Map::new(),
        Some(Value::Object(models)) => models.clone(),
        // Чужая форма: не трогаем.
        Some(_) => return Ok(false),
    };
    for (key, window) in keys {
        match (next, prev) {
            (Some(spec), _) => {
                let entry = models
                    .entry(key)
                    .or_insert_with(|| Value::Object(Map::new()));
                if let Value::Object(entry) = entry {
                    entry.insert("autoCompactWindow".into(), tokens(spec, window).into());
                }
            }
            (None, Some(old)) => {
                let Some(Value::Object(entry)) = models.get_mut(&key) else {
                    continue;
                };
                if entry.get("autoCompactWindow") == Some(&Value::from(tokens(old, window))) {
                    entry.shift_remove("autoCompactWindow");
                }
                if entry.is_empty() {
                    models.shift_remove(&key);
                }
            }
            (None, None) => {}
        }
    }
    // insert на месте старого ключа: порядок остальных не меняется.
    if models.is_empty() {
        map.shift_remove("modelSettings");
    } else {
        map.insert("modelSettings".into(), Value::Object(models));
    }
    if map == before {
        return Ok(false);
    }
    let mut text =
        serde_json::to_string_pretty(&Value::Object(map)).map_err(|exc| exc.to_string())?;
    if newline {
        text.push('\n');
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|exc| exc.to_string())?;
    }
    crate::store::write_file(path, &text)?;
    Ok(true)
}

/// Общий `settings.json` Claude Code; изолированный запуск его не трогает.
pub fn sync(config: &Config, prev: Option<&str>) {
    if crate::codemodel::isolated() {
        return;
    }
    let Some(path) = crate::codemodel::user_settings() else {
        return;
    };
    match sync_at(&path, config, prev) {
        Ok(true) => tracing::info!("compact window written to Claude Code settings"),
        Ok(false) => {}
        Err(exc) => tracing::warn!(error = %exc, "compact window not written"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ProviderRow;
    use serde_json::json;
    use std::fs;

    fn setup(compact: Option<&str>) -> (std::path::PathBuf, std::path::PathBuf, Config, String) {
        let dir = std::env::temp_dir()
            .join("agents")
            .join(format!("ctp-compact-{}", uuid::Uuid::new_v4().simple()));
        fs::create_dir_all(&dir).unwrap();
        let mut config = crate::store::load_or_create(&dir).unwrap();
        config.providers = vec![ProviderRow {
            id: "opencode".into(),
            kind: "openai".into(),
            base_url: "https://opencode.ai/v1".into(),
            models: vec!["glm-5.3[1m]".into()],
            ..Default::default()
        }];
        config.compact_window = compact.map(str::to_string);
        let alias = crate::store::alias_rows(&config.providers[0])[0]
            .alias
            .clone();
        (dir.join("settings.json"), dir, config, alias)
    }

    fn read(path: &Path) -> Value {
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn the_field_takes_tokens_and_percent_and_nothing_else() {
        assert_eq!(parse("400k"), Some(Spec::Tokens(400_000)));
        assert_eq!(parse(" 400000 "), Some(Spec::Tokens(400_000)));
        assert_eq!(parse("0.5M"), Some(Spec::Tokens(500_000)));
        assert_eq!(parse("2m"), Some(Spec::Tokens(MAX)));
        assert_eq!(parse("40%"), Some(Spec::Percent(40)));
        for bad in [
            "", "abc", "99k", "50000", "0%", "101%", "-5%", "k", "%", "NaN",
        ] {
            assert_eq!(parse(bad), None, "{bad:?}");
        }
        assert_eq!(tokens(Spec::Percent(40), MAX), 400_000);
        assert_eq!(
            tokens(Spec::Percent(40), PLAIN_WINDOW),
            MIN,
            "не ниже минимума"
        );
        assert_eq!(tokens(Spec::Percent(100), MAX), MAX);
    }

    #[test]
    fn both_model_keys_are_written_and_the_rest_of_the_file_stays() {
        let (path, dir, config, alias) = setup(Some("400k"));
        fs::write(
            &path,
            "{\n  \"theme\": \"dark\",\n  \"modelSettings\": {\n    \"opus\": {\n      \"effortLevel\": \"high\"\n    }\n  },\n  \"language\": \"Russian\"\n}\n",
        )
        .unwrap();
        assert!(sync_at(&path, &config, None).unwrap());
        let after = read(&path);
        // Ключ с `[1m]` — тот, что Claude Code 2.1.330 читает у модели с `[1m]`.
        assert_eq!(
            after["modelSettings"][format!("{alias}[1m]")]["autoCompactWindow"],
            400_000
        );
        assert_eq!(after["modelSettings"][&alias]["autoCompactWindow"], 400_000);
        assert_eq!(
            after["modelSettings"]["opus"],
            json!({"effortLevel": "high"})
        );
        assert_eq!(after["theme"], "dark");
        assert!(fs::read_to_string(&path).unwrap().ends_with('\n'));
        // Второй раз то же самое — файл не переписывается.
        assert!(!sync_at(&path, &config, None).unwrap());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_percent_is_counted_from_the_window_of_each_menu_row() {
        let (path, dir, config, alias) = setup(Some("50%"));
        assert!(sync_at(&path, &config, None).unwrap());
        let after = read(&path);
        assert_eq!(
            after["modelSettings"][format!("{alias}[1m]")]["autoCompactWindow"],
            500_000
        );
        assert_eq!(after["modelSettings"][&alias]["autoCompactWindow"], MIN);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn clearing_the_field_removes_only_what_it_wrote() {
        let (path, dir, mut config, alias) = setup(Some("400k"));
        fs::write(&path, "{\"theme\":\"dark\"}").unwrap();
        sync_at(&path, &config, None).unwrap();
        // Человек сам задал другое значение для обычной строки.
        let mut file = read(&path);
        file["modelSettings"][&alias]["autoCompactWindow"] = json!(300_000);
        file["modelSettings"][&alias]["effortLevel"] = json!("high");
        fs::write(&path, file.to_string()).unwrap();
        config.compact_window = None;
        assert!(sync_at(&path, &config, Some("400k")).unwrap());
        let after = read(&path);
        assert!(after["modelSettings"].get(format!("{alias}[1m]")).is_none());
        assert_eq!(
            after["modelSettings"][&alias],
            json!({"autoCompactWindow": 300_000, "effortLevel": "high"}),
            "чужое остаётся"
        );
        assert_eq!(after["theme"], "dark");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_empty_field_leaves_the_file_and_foreign_forms_alone() {
        let (path, dir, mut config, _) = setup(None);
        // Поле не задано и не было: файл не создаётся.
        assert!(!sync_at(&path, &config, None).unwrap());
        assert!(!path.exists());
        // Не объект и чужая форма `modelSettings` не трогаются.
        config.compact_window = Some("400k".into());
        for text in ["{ broken", "[1]", r#"{"modelSettings":[]}"#] {
            fs::write(&path, text).unwrap();
            let _ = sync_at(&path, &config, None);
            assert_eq!(fs::read_to_string(&path).unwrap(), text);
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_console_gets_tokens_counted_from_its_own_window() {
        let (_, dir, mut config, _) = setup(Some("40%"));
        assert_eq!(console_tokens(&config, MAX), Some(400_000));
        assert_eq!(console_tokens(&config, 500_000), Some(200_000));
        config.compact_window = Some("300k".into());
        assert_eq!(console_tokens(&config, MAX), Some(300_000));
        config.compact_window = None;
        assert_eq!(console_tokens(&config, MAX), None);
        fs::remove_dir_all(dir).unwrap();
    }
}
