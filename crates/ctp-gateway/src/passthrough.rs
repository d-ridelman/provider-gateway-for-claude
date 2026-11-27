use crate::error;
use crate::state::{AppState, Provider};
use crate::upstream::{Failure, Raw};
use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use futures_util::StreamExt;
use reqwest::RequestBuilder;
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use tracing::{info, warn};

pub(crate) const ANTHROPIC_VERSION: &str = "2023-06-01";

pub(crate) async fn messages(
    state: &AppState,
    provider: &Provider,
    uri: &Uri,
    incoming: &HeaderMap,
    mut body: Value,
    stream: bool,
    started: Instant,
) -> Result<Response, Failure> {
    body["stream"] = json!(stream);
    entrypoint_as_sdk_cli(&mut body);
    let url = provider.anthropic_url("/messages", uri.query());
    let request =
        upstream_headers(state.clients.stream.post(&url), provider, incoming, &body).json(&body);

    let upstream = match request.send().await {
        Ok(response) => response,
        Err(exc) => {
            warn!(provider = %provider.id, error = %exc, "Anthropic passthrough: network");
            return Err(Failure::Network(exc.to_string()));
        }
    };
    let status = upstream.status();

    if !stream || status != reqwest::StatusCode::OK {
        let content_type = upstream
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("application/json")
            .to_string();
        let relayed = relayed_headers(upstream.headers());
        let retry_after = upstream
            .headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let payload = match upstream.bytes().await {
            Ok(payload) => payload,
            Err(exc) => return Err(Failure::Body(exc.to_string())),
        };
        if !status.is_success() {
            // Anthropic-совместимые провайдеры не Anthropic пишут о переполнении окна своими
            // словами, а Claude Code сжимает разговор только по «prompt is too long».
            let text = String::from_utf8_lossy(&payload);
            let overflow =
                crate::openai::adapt::is_overflow(&text) && !text.contains("prompt is too long");
            // Провайдер или прокси может повторить ключ в тексте отказа; в окно Claude и в его
            // журнал он не должен попасть. В байтах, а не в тексте: тело не в UTF-8 (HTML
            // прокси в cp1251) остаётся целым.
            let text = error::without_secret(&text, &provider.api_key);
            let raw = (!overflow).then(|| {
                Box::new(Raw {
                    status: status.as_u16(),
                    content_type,
                    payload: error::without_secret_bytes(&payload, &provider.api_key).into(),
                    headers: relayed,
                })
            });
            info!(
                provider = %provider.id,
                status = status.as_u16(),
                ms = started.elapsed().as_millis(),
                "POST /v1/messages passthrough"
            );
            return Err(Failure::Status {
                status: status.as_u16(),
                text,
                // Переполнение сегодня уходит словами Anthropic и без заголовков.
                retry_after: if overflow { None } else { retry_after },
                raw,
            });
        }
        info!(
            provider = %provider.id,
            status = status.as_u16(),
            ms = started.elapsed().as_millis(),
            "POST /v1/messages passthrough"
        );
        let mut response = bytes_response(status.as_u16(), &content_type, payload);
        response.headers_mut().extend(relayed);
        return Ok(response);
    }

    info!(provider = %provider.id, "POST /v1/messages passthrough: stream");
    let relayed = relayed_headers(upstream.headers());
    let log = RequestLog::new(started, provider.id.clone(), "passthrough");
    let stream_body = async_stream::stream! {
        let _log = log;
        let mut bytes = upstream.bytes_stream();
        while let Some(item) = bytes.next().await {
            yield item;
        }
    };
    let mut response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from_stream(stream_body))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
    response.headers_mut().extend(relayed);
    Ok(response)
}

/// Счётчик токенов Anthropic-провайдера. Ошибку сети вызывающий превращает
/// в локальную оценку, как Python-шлюз.
pub async fn count_tokens(
    state: &AppState,
    provider: &Provider,
    incoming: &HeaderMap,
    body: &Value,
) -> anyhow::Result<Response> {
    let url = provider.anthropic_url("/messages/count_tokens", None);
    let request =
        upstream_headers(state.clients.json.post(&url), provider, incoming, body).json(body);
    let upstream = tokio::time::timeout(Duration::from_secs(120), request.send())
        .await
        .map_err(|_| anyhow::anyhow!("count_tokens: 120 s timeout"))??;

    let status = upstream.status();
    // Нет счётчика (404) или он отказал: Claude всё равно получит число, локальную оценку.
    if !status.is_success() {
        anyhow::bail!("count_tokens: HTTP {status}");
    }
    let content_type = upstream
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/json")
        .to_string();
    let payload = upstream.bytes().await?;
    Ok(bytes_response(status.as_u16(), &content_type, payload))
}

/// Заголовки запроса к провайдеру. Свои заголовки провайдера (`headers` в его настройках)
/// заменяют присланные Claude: так можно подменить `anthropic-beta`, который провайдер
/// отвергает, а пустое значение убирает заголовок совсем. Неверный — пропускается.
fn upstream_headers(
    request: RequestBuilder,
    provider: &Provider,
    incoming: &HeaderMap,
    body: &Value,
) -> RequestBuilder {
    let mut headers = HeaderMap::new();
    let version = incoming
        .get("anthropic-version")
        .cloned()
        .unwrap_or(HeaderValue::from_static(ANTHROPIC_VERSION));
    headers.insert("anthropic-version", version);
    if !provider.api_key.is_empty()
        && let Ok(value) = HeaderValue::from_str(&provider.api_key)
    {
        headers.insert("x-api-key", value);
    }
    for name in [
        "anthropic-beta",
        "accept",
        "user-agent",
        "x-claude-code-session-id",
    ] {
        if let Some(value) = incoming.get(name) {
            headers.insert(name, value.clone());
        }
    }
    // Anthropic отвечает «credit balance is too low» ключу API, чей user-agent — интерактивный
    // Claude Code (`cli`, `claude-desktop-3p` у вкладки Code, без метки); `sdk-*` пропускает.
    // Claude Code уходит меткой `claude -p` (ADR-008).
    if let Some(agent) = headers.get("user-agent").and_then(|v| v.to_str().ok())
        && let Some(version) = agent
            .strip_prefix("claude-cli/")
            .map(|rest| rest.split([' ', '(']).next().unwrap_or_default())
        && !agent.contains(", sdk-")
        && let Ok(value) =
            HeaderValue::from_str(&format!("claude-cli/{version} (external, sdk-cli)"))
    {
        headers.insert("user-agent", value);
    }
    if let Some((name, value)) =
        provider.session_pair(crate::state::session_id(incoming, body).as_deref())
    {
        headers.insert(name, value);
    }
    for (name, value) in &provider.headers {
        let Ok(name) = HeaderName::from_bytes(name.as_bytes()) else {
            warn!(header = %name, "invalid provider header skipped");
            continue;
        };
        if value.is_empty() {
            headers.remove(&name);
        } else if let Ok(value) = HeaderValue::from_str(value) {
            headers.insert(name, value);
        } else {
            warn!(header = %name, "invalid provider header skipped");
        }
    }
    request.headers(headers)
}

/// Блок атрибуции Claude Code в начале `system` (`x-anthropic-billing-header: …;
/// cc_entrypoint=claude-desktop-3p;`) Anthropic читает так же, как `user-agent`: интерактивная
/// метка с ключом API даёт «credit balance is too low». Уходит меткой `claude -p` (ADR-008).
fn entrypoint_as_sdk_cli(body: &mut Value) {
    const HEADER: &str = "x-anthropic-billing-header:";
    let fix = |text: &str| -> Option<String> {
        let start = text.find("cc_entrypoint=")? + "cc_entrypoint=".len();
        let end = text[start..].find(';').map_or(text.len(), |i| start + i);
        (!text[start..end].starts_with("sdk-"))
            .then(|| format!("{}sdk-cli{}", &text[..start], &text[end..]))
    };
    match body.get_mut("system") {
        Some(Value::String(text)) if text.starts_with(HEADER) => {
            if let Some(new) = fix(text) {
                *text = new;
            }
        }
        Some(Value::Array(blocks)) => {
            for block in blocks {
                if let Some(Value::String(text)) = block.get_mut("text")
                    && text.starts_with(HEADER)
                    && let Some(new) = fix(text)
                {
                    *text = new;
                }
            }
        }
        _ => {}
    }
}

/// Заголовки ответа провайдера, которые Claude Code и Desktop читают сами: когда повторять,
/// лимиты и `request-id` (по нему Desktop решает, переходить ли на свою проверку auto mode).
fn relayed_headers(from: &HeaderMap) -> HeaderMap {
    let mut out = HeaderMap::new();
    for (name, value) in from {
        let name_text = name.as_str();
        if matches!(name_text, "retry-after" | "x-should-retry" | "request-id")
            || name_text.starts_with("anthropic-ratelimit-")
        {
            out.append(name.clone(), value.clone());
        }
    }
    out
}

pub fn bytes_response(status: u16, content_type: &str, payload: Bytes) -> Response {
    let status = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
    let content_type = HeaderValue::from_str(content_type)
        .unwrap_or_else(|_| HeaderValue::from_static("application/json"));
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(payload))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

pub struct RequestLog {
    started: Instant,
    provider: String,
    operation: &'static str,
}

impl RequestLog {
    pub fn new(started: Instant, provider: String, operation: &'static str) -> Self {
        Self {
            started,
            provider,
            operation,
        }
    }
}

impl Drop for RequestLog {
    fn drop(&mut self) {
        info!(
            provider = %self.provider,
            ms = self.started.elapsed().as_millis(),
            "{} finished",
            self.operation
        );
    }
}
