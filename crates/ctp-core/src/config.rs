use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

/// `PROVIDERS_JSON` — то, что лаунчер уже пишет для Python-шлюза.
#[derive(Clone, Debug, Deserialize)]
pub struct ProvidersFile {
    #[serde(default)]
    pub default: Option<String>,
    #[serde(default)]
    pub providers: Vec<ProviderConfig>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ProviderConfig {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
    #[serde(default, alias = "baseUrl")]
    pub base_url: String,
    #[serde(default, alias = "apiKey")]
    pub api_key: Option<String>,
    /// Псевдоним → ID модели. Массив тоже принимается: `["m"]` → `{"m": "m"}`.
    #[serde(default, deserialize_with = "deserialize_models")]
    pub models: Vec<(String, String)>,
    /// Дополнительные заголовки к провайдеру (OpenRouter, Azure и т. п.).
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// Полный URL `/chat/completions`, если его нельзя вывести из `base_url`.
    #[serde(default, alias = "chatCompletionsUrl")]
    pub chat_completions_url: Option<String>,
    /// `max_tokens` | `max_completion_tokens` | `none`. По умолчанию — по имени модели.
    #[serde(default, alias = "maxTokensField")]
    pub max_tokens_field: Option<String>,
    /// Поля, которые провайдер хочет видеть в теле (OpenRouter reasoning и т. п.).
    #[serde(default, alias = "extraBody")]
    pub extra_body: Option<serde_json::Map<String, Value>>,
    /// Просить ли `stream_options.include_usage`; по умолчанию нет.
    #[serde(default, alias = "includeUsage")]
    pub include_usage: Option<bool>,
    /// Верхний уровень рассуждения (`low|medium|high|xhigh|max`). Задан — шлюз передаёт
    /// уровень из запроса Claude (`output_config.effort`) как `reasoning_effort`, не выше
    /// этого; не задан — `reasoning_effort` не отправляется вовсе.
    #[serde(default, alias = "maxEffort")]
    pub max_effort: Option<String>,
    /// Уровень, если запрос его не указал (терминальный Claude Code без `--effort`).
    #[serde(default, alias = "defaultEffort")]
    pub default_effort: Option<String>,
    /// Заголовок, в котором провайдер ждёт ID разговора (OpenCode Go: `x-opencode-session`).
    /// Шлюз кладёт туда ID сессии Claude Code. Для `opencode.ai` подставляется сам.
    #[serde(default, alias = "sessionHeader")]
    pub session_header: Option<String>,
}

fn deserialize_models<'de, D>(deserializer: D) -> Result<Vec<(String, String)>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    match value {
        Value::Null => Ok(Vec::new()),
        Value::Object(map) => {
            let mut out = Vec::with_capacity(map.len());
            for (alias, model) in map {
                let model = model.as_str().ok_or_else(|| {
                    serde::de::Error::custom(format!("model for alias {alias:?} is not a string"))
                })?;
                out.push((alias, model.to_string()));
            }
            Ok(out)
        }
        Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                let model = item
                    .as_str()
                    .ok_or_else(|| serde::de::Error::custom("models array must contain strings"))?;
                out.push((model.to_string(), model.to_string()));
            }
            Ok(out)
        }
        _ => Err(serde::de::Error::custom(
            "models must be an object (alias -> model) or an array of names",
        )),
    }
}

impl ProviderConfig {
    pub fn name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }

    pub fn is_anthropic(&self) -> bool {
        self.kind
            .as_deref()
            .is_some_and(|kind| kind.eq_ignore_ascii_case("anthropic"))
    }

    pub fn api_key(&self) -> &str {
        self.api_key.as_deref().unwrap_or("")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_providers_json() {
        // Незнакомые поля (здесь `extra_field`) пропускаются, а не роняют разбор.
        let json = r#"{
            "default": "claude-openai-80673633",
            "providers": [{
                "id": "openai", "name": "OpenAI", "type": "openai",
                "base_url": "https://api.openai.com/v1", "api_key": "sk-test",
                "models": {"claude-openai-80673633": "gpt-5.2"},
                "extra_field": false
            }]
        }"#;
        let cfg: ProvidersFile = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.providers.len(), 1);
        assert_eq!(cfg.providers[0].models.len(), 1);
        assert_eq!(cfg.providers[0].api_key(), "sk-test");
        assert!(!cfg.providers[0].is_anthropic());
    }

    #[test]
    fn accepts_camel_case_and_model_array() {
        let json = r#"{"providers":[{
            "id":"ollama","baseUrl":"http://127.0.0.1:11434/v1",
            "models":["qwen3:8b"]
        }]}"#;
        let cfg: ProvidersFile = serde_json::from_str(json).unwrap();
        assert_eq!(
            cfg.providers[0].models,
            vec![("qwen3:8b".into(), "qwen3:8b".into())]
        );
    }
}
