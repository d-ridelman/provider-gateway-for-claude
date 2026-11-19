//! Проверки провайдера для окна приложения: список моделей у провайдера и
//! пробный запрос через тот же шлюз, что обслуживает Claude.

use crate::error::{clean_error_text, truncate_redacted, without_secret};
use crate::state::{AppState, ModelRouter, Provider, ProviderKind};
use axum::body::Body;
use axum::http::Request;
use ctp_core::config::ProvidersFile;
use serde_json::{Value, json};
use std::time::Duration;
use tower::ServiceExt;

const LIST_TIMEOUT: Duration = Duration::from_secs(20);
const CHAT_TIMEOUT: Duration = Duration::from_secs(90);

/// ID моделей, которые отдаёт `GET …/models` провайдера, по алфавиту.
pub async fn list_models(provider: &Provider) -> Result<Vec<String>, String> {
    let client = reqwest::Client::builder()
        .user_agent(crate::state::USER_AGENT)
        .connect_timeout(LIST_TIMEOUT)
        .timeout(LIST_TIMEOUT)
        .build()
        .map_err(|exc| exc.to_string())?;
    let url = provider.models_url();
    let request = match provider.kind {
        ProviderKind::OpenAi => client
            .get(&url)
            .headers(crate::openai::upstream_headers(provider, None)),
        ProviderKind::Anthropic => {
            // У Anthropic список постраничный, по умолчанию 20 штук.
            let mut request = client
                .get(&url)
                .query(&[("limit", "1000")])
                .header("anthropic-version", crate::passthrough::ANTHROPIC_VERSION);
            if !provider.api_key.is_empty() {
                request = request.header("x-api-key", &provider.api_key);
            }
            if let Some((name, value)) = provider.session_pair(None) {
                request = request.header(name, value);
            }
            for (name, value) in &provider.headers {
                request = request.header(name, value);
            }
            request
        }
    };
    let keys = [provider.api_key.as_str()];
    let response = request.send().await.map_err(|exc| {
        // У reqwest верхний текст ошибки — «error sending request», суть в причине.
        let why = if exc.is_timeout() {
            format!("no response in {} s", LIST_TIMEOUT.as_secs())
        } else {
            // Самая глубокая причина: «отказано в подключении», ошибка сертификата и т. п.
            let mut cause: &dyn std::error::Error = &exc;
            while let Some(next) = cause.source() {
                cause = next;
            }
            if exc.is_connect() {
                format!("could not connect (address, port, VPN/proxy?): {cause}")
            } else {
                cause.to_string()
            }
        };
        scrub(&format!("{url}: {why}"), &keys)
    })?;
    let status = response.status();
    let raw = response.text().await.map_err(|exc| exc.to_string())?;
    // Ключ вырезаем из сырого тела, до разбора ошибки и любых обрезок.
    let text = without_secret(&raw, &provider.api_key);
    if !status.is_success() {
        let message = clean_error_text(&raw, &provider.api_key);
        return Err(scrub(
            &format!("HTTP {}: {message}", status.as_u16()),
            &keys,
        ));
    }
    parse_model_ids(&text).map_err(|exc| scrub(&exc, &keys))
}

/// Текст для окна: обрезан, без `sk-…`/`Bearer …` и без самих ключей провайдера,
/// какой бы формы они ни были (Gemini `AIza…` и т. п.). Вход должен быть ещё не обрезан.
fn scrub(text: &str, keys: &[&str]) -> String {
    let text = keys
        .iter()
        .fold(text.to_string(), |text, key| without_secret(&text, key));
    truncate_redacted(&text)
}

fn parse_model_ids(text: &str) -> Result<Vec<String>, String> {
    let value: Value = serde_json::from_str(text).map_err(|_| {
        let head: String = truncate_redacted(text).chars().take(200).collect();
        format!("response does not look like a model list: {head}")
    })?;
    let items = value
        .get("data")
        .or_else(|| value.get("models"))
        .unwrap_or(&value)
        .as_array()
        .ok_or_else(|| "no model list in the response".to_string())?;
    let mut ids: Vec<String> = items
        .iter()
        .filter_map(|item| {
            item.as_str()
                .or_else(|| item.get("id").and_then(Value::as_str))
                .or_else(|| item.get("name").and_then(Value::as_str))
        })
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
        .collect();
    ids.sort();
    ids.dedup();
    Ok(ids)
}

/// Настоящий короткий запрос `/v1/messages` через шлюз: проверяет ключ,
/// модель и перевод запроса разом. `model` — псевдоним из `file`.
/// Форма запроса намеренно не похожа на probe Claude (`max_tokens: 1`, `.`),
/// иначе шлюз ответит сам, не спрашивая провайдера.
pub async fn chat_check(file: ProvidersFile, model: &str) -> Result<String, String> {
    let owned: Vec<String> = file
        .providers
        .iter()
        .map(|p| p.api_key().to_string())
        .collect();
    let keys: Vec<&str> = owned.iter().map(String::as_str).collect();
    let router = ModelRouter::new(file).map_err(|exc| exc.to_string())?;
    let state = AppState::new(router, "probe".into(), false, String::new())
        .map_err(|exc| exc.to_string())?;
    let body = json!({
        "model": model,
        "max_tokens": 256,
        "messages": [{"role": "user", "content": "Reply with the single word OK."}]
    });
    let request = Request::post("/v1/messages")
        .header("x-api-key", "probe")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .map_err(|exc| exc.to_string())?;
    let response = tokio::time::timeout(CHAT_TIMEOUT, crate::app(state).oneshot(request))
        .await
        .map_err(|_| format!("provider did not respond in {} s", CHAT_TIMEOUT.as_secs()))?
        .map_err(|exc| exc.to_string())?;
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .map_err(|exc| exc.to_string())?;
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    if !status.is_success() {
        // Anthropic passthrough отдаёт тело провайдера как есть — чистим здесь.
        let message = value
            .pointer("/error/message")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| String::from_utf8_lossy(&bytes).into_owned());
        return Err(scrub(
            &format!("HTTP {}: {message}", status.as_u16()),
            &keys,
        ));
    }
    let text: String = value["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect();
    let text = text.trim();
    if text.is_empty() {
        let stop = value["stop_reason"].as_str().unwrap_or("?");
        return Ok(format!("empty response (stop_reason: {stop})"));
    }
    Ok(scrub(text, &keys).chars().take(200).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_openai_anthropic_and_bare_lists() {
        let openai = r#"{"object":"list","data":[{"id":"gpt-5.2"},{"id":"a"},{"id":"a"}]}"#;
        assert_eq!(parse_model_ids(openai).unwrap(), vec!["a", "gpt-5.2"]);
        let bare = r#"["m1", {"name": "m2"}]"#;
        assert_eq!(parse_model_ids(bare).unwrap(), vec!["m1", "m2"]);
        assert!(parse_model_ids("<html>nope</html>").is_err());
        assert!(parse_model_ids(r#"{"ok":true}"#).is_err());
    }

    #[test]
    fn scrub_removes_whole_key_even_where_regex_or_cut_would_split_it() {
        let odd = "sk-abc+def/ghi=jkl"; // regex съел бы только начало до «+»
        assert!(!scrub(&format!("bad key {odd}"), &[odd]).contains("def/ghi"));
        let long = "AIzaSyLongKeyAtTheCutBoundary";
        let text = format!("{}{long}", "x".repeat(1990));
        let out = scrub(&text, &[long]);
        assert!(!out.contains("AIzaSy"), "{out}");
    }
}
