//! MCP-серверы обычного Claude Desktop → профиль PGC (идея из Loadout: перенос записи между
//! клиентами с предпросмотром и копией).
//!
//! Обычный Claude (`%APPDATA%\Claude\claude_desktop_config.json`) только читается. В профиль
//! PGC (`Claude-3p\claude_desktop_config.json`) записи копируются как есть, включая `env`
//! (ключи серверов) — в окне видны только имя и программа. Пока Claude из PGC открыт, он
//! сам пишет этот файл, поэтому перенос ждёт его закрытия; прежний файл остаётся копией рядом.

use crate::tr;
use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Сервер обычного Claude для окна.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Server {
    pub name: String,
    /// Что запускается: программа (без аргументов — в них бывают ключи) или хост адреса.
    pub what: String,
    /// Такой уже есть в профиле PGC.
    pub present: bool,
}

/// Конфиг обычного Claude Desktop (Roaming, не AppData\Local).
fn regular() -> Option<PathBuf> {
    Some(
        dirs::config_dir()?
            .join("Claude")
            .join("claude_desktop_config.json"),
    )
}

fn profile() -> Option<PathBuf> {
    Some(crate::claude::base_dir()?.join("Claude-3p"))
}

fn servers_of(path: &Path) -> serde_json::Map<String, Value> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}')).ok())
        .and_then(|config| config.get("mcpServers").and_then(Value::as_object).cloned())
        .unwrap_or_default()
}

/// Что показать: программа (имя файла) или хост удалённого сервера.
pub fn what(server: &Value) -> String {
    if let Some(url) = server.get("url").and_then(Value::as_str) {
        return crate::mihomo::host_of(url);
    }
    let command = server.get("command").and_then(Value::as_str).unwrap_or("");
    Path::new(command)
        .file_name()
        .map_or_else(|| command.to_string(), |n| n.to_string_lossy().into_owned())
}

pub fn list() -> Vec<Server> {
    let Some(source) = regular() else {
        return Vec::new();
    };
    let present = profile()
        .map(|p| servers_of(&p.join("claude_desktop_config.json")))
        .unwrap_or_default();
    servers_of(&source)
        .iter()
        .map(|(name, server)| Server {
            name: name.clone(),
            what: what(server),
            present: present.contains_key(name),
        })
        .collect()
}

/// Добавляет в набор `target` серверы `names` из `source`; уже существующие не трогает.
/// Ответ — сколько добавлено.
pub fn merge(
    target: &mut serde_json::Map<String, Value>,
    source: &serde_json::Map<String, Value>,
    names: &[String],
) -> usize {
    let mut added = 0;
    for name in names {
        if let Some(server) = source.get(name)
            && !target.contains_key(name)
        {
            target.insert(name.clone(), server.clone());
            added += 1;
        }
    }
    added
}

/// Переносит выбранные серверы в профиль PGC. Ответ — текст для окна.
pub fn copy(names: &[String]) -> Result<String, String> {
    let (Some(source), Some(profile)) = (regular(), profile()) else {
        return Err(tr!("Не найдена папка профиля", "Profile folder not found"));
    };
    if crate::claude::profile_in_use(&profile) {
        return Err(tr!(
            "Claude из PGC открыт и сам пишет свои настройки — закройте его и повторите",
            "Claude from PGC is open and writes its own settings; close it and try again"
        ));
    }
    let source = servers_of(&source);
    let target = profile.join("claude_desktop_config.json");
    std::fs::create_dir_all(&profile).map_err(|exc| exc.to_string())?;
    // Копия прежнего файла рядом: перенос можно откатить, вернув её на место.
    let backup = target.with_extension("json.before-mcp");
    if target.is_file() {
        std::fs::copy(&target, &backup).map_err(|exc| format!("{}: {exc}", backup.display()))?;
    }
    let mut added = 0;
    crate::claude::update_desktop_config(&target, |config| {
        if !config.get("mcpServers").is_some_and(Value::is_object) {
            config["mcpServers"] = Value::Object(Default::default());
        }
        if let Some(servers) = config["mcpServers"].as_object_mut() {
            added = merge(servers, &source, names);
        }
    })?;
    let shown = backup.display();
    Ok(if added == 0 {
        tr!(
            "Ничего не перенесено: эти серверы уже есть в профиле PGC",
            "Nothing copied: these servers are already in the PGC profile"
        )
    } else {
        tr!(
            "Перенесено серверов: {added}. Подхватит следующее «Открыть Claude». Прежний файл: {shown}",
            "Servers copied: {added}. The next Open Claude picks them up. The previous file: {shown}"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn merge_adds_only_chosen_and_missing_servers() {
        let source = json!({
            "a": {"command": "npx", "args": ["-y", "a"], "env": {"KEY": "secret"}},
            "b": {"command": "C:\\tools\\b.exe"},
            "c": {"url": "https://mcp.example.com/sse?token=x"},
        });
        let source = source.as_object().unwrap();
        let mut target = json!({"b": {"command": "mine"}})
            .as_object()
            .unwrap()
            .clone();
        let added = merge(&mut target, source, &["a".into(), "b".into(), "zzz".into()]);
        assert_eq!(added, 1);
        assert_eq!(target["a"]["env"]["KEY"], "secret", "copied whole");
        assert_eq!(target["b"]["command"], "mine", "an existing one stays");
        assert!(!target.contains_key("c"));
        assert_eq!(what(&source["b"]), "b.exe");
        assert_eq!(what(&source["c"]), "mcp.example.com");
        assert_eq!(what(&source["a"]), "npx");
    }
}
