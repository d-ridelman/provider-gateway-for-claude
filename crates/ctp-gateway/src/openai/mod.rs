use crate::error;
use crate::passthrough::RequestLog;
use crate::sse;
use crate::state::{AppState, Provider};
use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::convert::Infallible;
use std::time::Instant;
use tracing::{info, warn};

pub mod request;
pub mod response;
pub mod stream;

#[allow(clippy::too_many_arguments)]
pub async fn messages(
    state: &AppState,
    provider: &Provider,
    client_model: &str,
    upstream_model: &str,
    body: Value,
    stream: bool,
    started: Instant,
    session: Option<&str>,
) -> Response {
    let url = provider.chat_url();
    let mut payload = request::build(provider, &body, stream);
    let headers = upstream_headers(provider, session);
    let input_tokens = ctp_core::tokens::count_json_tokens(&body);

    // Один повтор, если провайдер ругается на слишком большой max_tokens.
    let mut attempt = 0u8;
    let upstream = loop {
        let request = state
            .clients
            .stream
            .post(&url)
            .headers(headers.clone())
            .json(&payload);
        let response = match request.send().await {
            Ok(response) => response,
            Err(exc) => {
                warn!(provider = %provider.id, error = %exc, "OpenAI upstream: network");
                return error::anthropic_error(502, "api_error", exc.to_string());
            }
        };
        if response.status().is_success() {
            break response;
        }

        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let text = response.text().await.unwrap_or_default();
        if attempt == 0
            && let Some(limit) = crate::util::parse_output_limit(&text)
            && let Some(key) = provider.max_tokens_key(upstream_model)
            && let Some(current) = payload.get(key.as_str()).and_then(Value::as_u64)
            && current > limit
        {
            info!(
                provider = %provider.id,
                model = %upstream_model,
                from = current,
                to = limit,
                "max_tokens above the model limit, retrying"
            );
            payload[key.as_str()] = json!(limit);
            attempt = 1;
            continue;
        }
        warn!(
            provider = %provider.id,
            model = %upstream_model,
            status,
            "OpenAI upstream error"
        );
        let mut response = error::upstream_error(status, &text, &provider.api_key);
        if let Some(value) = retry_after
            && let Ok(value) = HeaderValue::from_str(&value)
        {
            response.headers_mut().insert("retry-after", value);
        }
        return response;
    };

    let status = upstream.status();
    if !stream {
        let text = match upstream.text().await {
            Ok(text) => text,
            Err(exc) => return error::anthropic_error(502, "api_error", exc.to_string()),
        };
        let value: Value = match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(_) => {
                return error::anthropic_error(502, "api_error", "provider returned non-JSON");
            }
        };
        if value.get("error").is_some() {
            return error::upstream_error(status.as_u16(), &text, &provider.api_key);
        }
        let message = match response::translate(&value, client_model, input_tokens) {
            Ok(message) => message,
            Err(exc) => return error::anthropic_error(502, "api_error", exc.to_string()),
        };
        info!(
            provider = %provider.id,
            model = %upstream_model,
            ms = started.elapsed().as_millis(),
            "POST /v1/messages openai"
        );
        return (StatusCode::OK, axum::Json(message)).into_response();
    }

    let content_type = upstream
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let is_sse = content_type.is_empty() || content_type.contains("text/event-stream");

    if !is_sse {
        // Провайдер проигнорировал stream и вернул обычный JSON.
        let text = match upstream.text().await {
            Ok(text) => text,
            Err(exc) => return error::anthropic_error(502, "api_error", exc.to_string()),
        };
        let value: Value = match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(_) => {
                return error::anthropic_error(502, "api_error", "provider returned non-JSON");
            }
        };
        if value.get("error").is_some() {
            return error::upstream_error(status.as_u16(), &text, &provider.api_key);
        }
        let message = match response::translate(&value, client_model, input_tokens) {
            Ok(message) => message,
            Err(exc) => return error::anthropic_error(502, "api_error", exc.to_string()),
        };
        info!(
            provider = %provider.id,
            model = %upstream_model,
            "POST /v1/messages openai: stream replaced with a whole response"
        );
        return sse::response(response::to_events(&message));
    }

    let mut translator = stream::StreamTranslator::new(client_model, input_tokens)
        .with_secret(provider.api_key.clone());
    let start = translator.start();
    let log = RequestLog::new(started, provider.id.clone(), "openai stream");
    let events = async_stream::stream! {
        let _log = log;
        yield Ok::<Bytes, Infallible>(sse::frame(&start));

        let mut source = upstream.bytes_stream().eventsource();
        let mut ticker = tokio::time::interval_at(
            tokio::time::Instant::now() + std::time::Duration::from_secs(15),
            std::time::Duration::from_secs(15),
        );
        loop {
            let item = tokio::select! {
                item = source.next() => item,
                _ = ticker.tick() => {
                    yield Ok::<Bytes, Infallible>(sse::frame(&json!({"type": "ping"})));
                    continue;
                }
            };
            let Some(item) = item else { break };
            let event = match item {
                Ok(event) => event,
                Err(exc) => {
                    yield Ok::<Bytes, Infallible>(sse::frame(
                        &stream::StreamTranslator::error_event(&format!("upstream stream: {exc}"))
                    ));
                    break;
                }
            };
            if event.data.trim() == "[DONE]" {
                break;
            }
            let chunk: Value = match serde_json::from_str(&event.data) {
                Ok(chunk) => chunk,
                Err(_) => {
                    yield Ok::<Bytes, Infallible>(sse::frame(
                        &stream::StreamTranslator::error_event("upstream sent invalid JSON")
                    ));
                    break;
                }
            };
            for value in translator.on_chunk(&chunk) {
                yield Ok::<Bytes, Infallible>(sse::frame(&value));
            }
            if translator.is_done() {
                break;
            }
        }

        if !translator.is_done() {
            for value in translator.finish() {
                yield Ok::<Bytes, Infallible>(sse::frame(&value));
            }
        }
    };

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from_stream(events))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

pub(crate) fn upstream_headers(provider: &Provider, session: Option<&str>) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    if !provider.api_key.is_empty()
        && let Ok(value) = HeaderValue::from_str(&format!("Bearer {}", provider.api_key))
    {
        headers.insert(header::AUTHORIZATION, value);
    }
    if let Some((name, value)) = provider.session_pair(session) {
        headers.insert(name, value);
    }
    for (name, value) in &provider.headers {
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(value),
        ) {
            headers.insert(name, value);
        } else {
            warn!(header = %name, "invalid provider header skipped");
        }
    }
    headers
}
