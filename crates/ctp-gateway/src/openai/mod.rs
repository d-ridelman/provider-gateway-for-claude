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

pub mod advisor;
pub mod request;
pub mod response;
pub mod stream;

/// Отказ провайдера до первого байта ответа.
enum SendError {
    Network(String),
    Status {
        status: u16,
        text: String,
        retry_after: Option<String>,
    },
}

impl SendError {
    fn response(self, provider: &Provider) -> Response {
        match self {
            Self::Network(text) => error::anthropic_error(502, "api_error", text),
            Self::Status {
                status,
                text,
                retry_after,
            } => {
                let mut response = error::upstream_error(status, &text, &provider.api_key);
                if let Some(value) = retry_after
                    && let Ok(value) = HeaderValue::from_str(&value)
                {
                    response.headers_mut().insert("retry-after", value);
                }
                response
            }
        }
    }

    /// Та же ошибка внутри уже начатого потока (после `message_start`).
    fn event(&self, provider: &Provider) -> Value {
        let text = match self {
            Self::Network(text) => text.clone(),
            Self::Status { text, .. } => error::clean_error_text(text, &provider.api_key),
        };
        stream::StreamTranslator::error_event(&text)
    }
}

/// Запрос к провайдеру; один повтор, если он ругается на слишком большой max_tokens.
async fn send(
    client: &reqwest::Client,
    provider: &Provider,
    headers: &HeaderMap,
    upstream_model: &str,
    payload: &mut Value,
) -> Result<reqwest::Response, SendError> {
    let url = provider.chat_url();
    let mut attempt = 0u8;
    loop {
        let request = client.post(&url).headers(headers.clone()).json(&*payload);
        let response = match request.send().await {
            Ok(response) => response,
            Err(exc) => {
                warn!(provider = %provider.id, error = %exc, "OpenAI upstream: network");
                return Err(SendError::Network(exc.to_string()));
            }
        };
        if response.status().is_success() {
            return Ok(response);
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
        return Err(SendError::Status {
            status,
            text,
            retry_after,
        });
    }
}

/// Вызов советника, пришедший не потоком (провайдер проигнорировал `stream`): исполнить
/// его негде — блок убирается, чтобы Claude Code не получил вызов незнакомого инструмента.
fn drop_advisor_calls(message: &mut Value) {
    let Some(content) = message.get_mut("content").and_then(Value::as_array_mut) else {
        return;
    };
    let before = content.len();
    content.retain(|block| {
        !(block.get("type").and_then(Value::as_str) == Some("tool_use")
            && block.get("name").and_then(Value::as_str) == Some(request::ADVISOR))
    });
    let tools_left = content
        .iter()
        .any(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"));
    if content.len() != before && !tools_left {
        message["stop_reason"] = json!("end_turn");
    }
}

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
    // Советника шлюз исполняет сам, и только в потоке: там ответ можно продолжить.
    let advisor_route = if stream {
        request::advisor_tool(&body).and_then(|named| state.router.advisor_for(named))
    } else {
        None
    };
    let mut payload = request::build(provider, &body, stream, advisor_route.is_some());
    let headers = upstream_headers(provider, session);
    let input_tokens = ctp_core::tokens::count_json_tokens(&body);

    let upstream = match send(
        &state.clients.stream,
        provider,
        &headers,
        upstream_model,
        &mut payload,
    )
    .await
    {
        Ok(upstream) => upstream,
        Err(exc) => return exc.response(provider),
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
        let mut message = match response::translate(&value, client_model, input_tokens) {
            Ok(message) => message,
            Err(exc) => return error::anthropic_error(502, "api_error", exc.to_string()),
        };
        if advisor_route.is_some() {
            drop_advisor_calls(&mut message);
        }
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
    // Для продолжений после советника поток живёт дольше этого вызова.
    let state = state.clone();
    let provider = provider.clone();
    let upstream_model = upstream_model.to_string();
    let session = session.map(str::to_string);
    let events = async_stream::stream! {
        let _log = log;
        yield Ok::<Bytes, Infallible>(sse::frame(&start));

        let mut upstream = upstream;
        let mut advisor_calls = 0u32;
        let mut ticker = tokio::time::interval_at(
            tokio::time::Instant::now() + std::time::Duration::from_secs(15),
            std::time::Duration::from_secs(15),
        );
        // Отрезки одного ответа: модель вызвала советника — шлюз спрашивает его и
        // продолжает тот же ответ с его советом.
        'segments: loop {
            let mut source = upstream.bytes_stream().eventsource();
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
                        break 'segments;
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
                        break 'segments;
                    }
                };
                for value in translator.on_chunk(&chunk) {
                    yield Ok::<Bytes, Infallible>(sse::frame(&value));
                }
                if translator.is_done() {
                    break 'segments;
                }
            }

            let Some(route) = advisor_route.as_ref() else { break };
            let Some((call, events)) = translator.take_advisor_call() else { break };
            for value in events {
                yield Ok::<Bytes, Infallible>(sse::frame(&value));
            }

            // Советник видит весь разговор и текст этого отрезка до вызова.
            let mut seen = payload["messages"].as_array().cloned().unwrap_or_default();
            if !call.text.is_empty() {
                seen.push(json!({"role": "assistant", "content": call.text}));
            }
            let consult = advisor::consult(&state, route, &seen, session.as_deref());
            tokio::pin!(consult);
            let result = loop {
                tokio::select! {
                    result = &mut consult => break result,
                    _ = ticker.tick() => {
                        yield Ok::<Bytes, Infallible>(sse::frame(&json!({"type": "ping"})));
                    }
                }
            };
            let content = match result {
                Ok(text) => json!({"type": "advisor_result", "text": text}),
                Err(code) => json!({"type": "advisor_tool_result_error", "error_code": code}),
            };
            let advice = request::advisor_text(&content);
            for value in translator.advisor_result(&call.server_id, content) {
                yield Ok::<Bytes, Infallible>(sse::frame(&value));
            }
            advisor_calls += 1;
            // Вместе с советником модель вызвала и инструменты Claude Code: их вызовы
            // уходят ему, ответ кончается — продолжит модель уже с их результатами.
            if call.others {
                break;
            }

            if let Some(messages) = payload["messages"].as_array_mut() {
                messages.push(json!({
                    "role": "assistant",
                    "content": if call.text.is_empty() { Value::Null } else { json!(call.text) },
                    "tool_calls": [{
                        "id": call.call_id,
                        "type": "function",
                        "function": {"name": request::ADVISOR, "arguments": "{}"}
                    }]
                }));
                messages.push(json!({"role": "tool", "tool_call_id": call.call_id, "content": advice}));
            }
            if advisor_calls >= advisor::MAX_CALLS
                && let Some(tools) = payload.get_mut("tools").and_then(Value::as_array_mut)
            {
                tools.retain(|tool| {
                    tool.pointer("/function/name").and_then(Value::as_str) != Some(request::ADVISOR)
                });
            }
            translator.next_segment();
            match send(&state.clients.stream, &provider, &headers, &upstream_model, &mut payload).await {
                Ok(next) => upstream = next,
                Err(exc) => {
                    yield Ok::<Bytes, Infallible>(sse::frame(&exc.event(&provider)));
                    break;
                }
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
