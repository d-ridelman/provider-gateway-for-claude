use crate::error;
use crate::state::{AppState, Provider};
use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use futures_util::StreamExt;
use reqwest::RequestBuilder;
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use tracing::{info, warn};

pub(crate) const ANTHROPIC_VERSION: &str = "2023-06-01";

pub async fn messages(
    state: &AppState,
    provider: &Provider,
    uri: &Uri,
    incoming: &HeaderMap,
    mut body: Value,
    stream: bool,
    started: Instant,
) -> Response {
    body["stream"] = json!(stream);
    let url = provider.anthropic_url("/messages", uri.query());
    let request =
        upstream_headers(state.clients.stream.post(&url), provider, incoming, &body).json(&body);

    let upstream = match request.send().await {
        Ok(response) => response,
        Err(exc) => {
            warn!(provider = %provider.id, error = %exc, "Anthropic passthrough: network");
            return error::anthropic_error(502, "api_error", exc.to_string());
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
        let retry_after = upstream
            .headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let mut payload = match upstream.bytes().await {
            Ok(payload) => payload,
            Err(exc) => return error::anthropic_error(502, "api_error", exc.to_string()),
        };
        // Провайдер или прокси может повторить ключ в тексте отказа; в окно Claude и в его
        // журнал он не должен попасть (OpenAI-путь вырезает его так же).
        if !status.is_success() {
            let text = String::from_utf8_lossy(&payload);
            payload = error::without_secret(&text, &provider.api_key).into();
        }
        info!(
            provider = %provider.id,
            status = status.as_u16(),
            ms = started.elapsed().as_millis(),
            "POST /v1/messages passthrough"
        );
        let mut response = bytes_response(status.as_u16(), &content_type, payload);
        if let Some(value) = retry_after
            && let Ok(value) = HeaderValue::from_str(&value)
        {
            response.headers_mut().insert("retry-after", value);
        }
        return response;
    }

    info!(provider = %provider.id, "POST /v1/messages passthrough: stream");
    let log = RequestLog::new(started, provider.id.clone(), "passthrough");
    let stream_body = async_stream::stream! {
        let _log = log;
        let mut bytes = upstream.bytes_stream();
        while let Some(item) = bytes.next().await {
            yield item;
        }
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from_stream(stream_body))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
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

fn upstream_headers(
    mut request: RequestBuilder,
    provider: &Provider,
    incoming: &HeaderMap,
    body: &Value,
) -> RequestBuilder {
    let version = incoming
        .get("anthropic-version")
        .and_then(|value| value.to_str().ok())
        .unwrap_or(ANTHROPIC_VERSION);
    request = request.header("anthropic-version", version);
    if !provider.api_key.is_empty() {
        request = request.header("x-api-key", &provider.api_key);
    }
    for name in [
        "anthropic-beta",
        "accept",
        "user-agent",
        "x-claude-code-session-id",
    ] {
        if let Some(value) = incoming.get(name).and_then(|value| value.to_str().ok()) {
            request = request.header(name, value);
        }
    }
    if let Some((name, value)) =
        provider.session_pair(crate::state::session_id(incoming, body).as_deref())
    {
        request = request.header(name, value);
    }
    for (name, value) in &provider.headers {
        request = request.header(name, value);
    }
    request
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
