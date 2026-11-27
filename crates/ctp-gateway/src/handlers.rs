use crate::auth;
use crate::error;
use crate::openai;
use crate::passthrough;
use crate::sse;
use crate::state::{AppState, Provider, ProviderKind, Route};
use crate::upstream::Failure;
use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use std::time::Instant;
use tracing::{info, warn};
use uuid::Uuid;

pub(crate) const AUTH_ERROR: &str = "invalid x-api-key";

pub async fn health(State(state): State<AppState>) -> Response {
    (
        StatusCode::OK,
        Json(json!({
            "ok": true,
            "version": env!("CARGO_PKG_VERSION"),
            "providers": state.router.providers.len(),
            "models": state.router.routes.len()
        })),
    )
        .into_response()
}

pub async fn models(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !auth::authorized(&headers, &state.token, state.open_mode) {
        return error::anthropic_error(401, "authentication_error", AUTH_ERROR);
    }

    let data: Vec<Value> = state
        .router
        .routes
        .iter()
        .map(|(alias, route)| {
            let provider = &state.router.providers[route.provider];
            json!({
                "type": "model",
                "id": alias,
                "display_name": format!("{} · {}", route.model, provider.name),
                "created_at": "2026-01-01T00:00:00Z"
            })
        })
        .collect();
    let first_id = data
        .first()
        .and_then(|item| item.get("id"))
        .cloned()
        .unwrap_or(Value::Null);
    let last_id = data
        .last()
        .and_then(|item| item.get("id"))
        .cloned()
        .unwrap_or(Value::Null);

    (
        StatusCode::OK,
        Json(json!({
            "data": data,
            "has_more": false,
            "first_id": first_id,
            "last_id": last_id
        })),
    )
        .into_response()
}

pub async fn messages(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    let started = Instant::now();
    if !auth::authorized(&headers, &state.token, state.open_mode) {
        return error::anthropic_error(401, "authentication_error", AUTH_ERROR);
    }

    let mut body: Value = match serde_json::from_slice(&body) {
        Ok(body) => body,
        Err(_) => {
            return error::anthropic_error(400, "invalid_request_error", "body is not JSON");
        }
    };
    let requested = body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let Some(route) = state.router.resolve(&requested) else {
        return error::anthropic_error(503, "api_error", "no models configured");
    };
    let provider = state.router.providers[route.provider].clone();
    let response_model = if requested.is_empty() {
        route.alias.clone()
    } else {
        requested
    };
    let upstream_model = route.model.clone();
    let stream = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    // Советника (`/advisor`) шлюз не исполняет: его объявление (остался `advisorModel` в
    // настройках Claude Code) и блоки в истории старых сессий провайдер бы не понял. До
    // распознавания пробы и `WebSearch`: те смотрят на список инструментов.
    strip_advisor(&mut body);

    if state.debug_prompts {
        let system = body
            .get("system")
            .map(summarize_content)
            .unwrap_or_default();
        let last = body
            .get("messages")
            .and_then(Value::as_array)
            .and_then(|messages| messages.last())
            .map(|message| summarize_content(message.get("content").unwrap_or(&Value::Null)))
            .unwrap_or_default();
        info!(
            provider = %provider.id,
            model = %upstream_model,
            max_tokens = body.get("max_tokens").and_then(|value| value.as_i64()).unwrap_or(0),
            stream,
            tools = body.get("tools").and_then(|value| value.as_array()).map(|tools| tools.len()).unwrap_or(0),
            system = %truncate_chars(&system, 90),
            last = %truncate_chars(&last, 140),
            "REQ"
        );
    }

    if is_probe(&body) {
        info!(
            provider = %provider.id,
            model = %upstream_model,
            "POST /v1/messages probe -> 200 (local)"
        );
        return probe_reply(&response_model, stream);
    }

    // Встроенный `WebSearch` Claude присылает вложенный запрос с серверным инструментом
    // `web_search`: провайдеры шлюза его не умеют, поэтому поиск выполняется здесь.
    if let Some(search) = crate::websearch_tool::detect(&body) {
        let language = crate::websearch::language_of(&headers);
        return crate::websearch_tool::reply(
            &state,
            &response_model,
            &body,
            search,
            stream,
            &language,
        )
        .await;
    }

    // Какой уровень рассуждения попросил Claude и что уйдёт провайдеру.
    let asked = body
        .pointer("/output_config/effort")
        .and_then(Value::as_str)
        .unwrap_or("-")
        .to_string();
    let level = provider.reasoning_effort(&body);
    let sent = match provider.kind {
        // extra_body провайдера главнее уровня из запроса (так и уходит в request::build).
        ProviderKind::OpenAi => provider
            .extra_body
            .get("reasoning_effort")
            .and_then(Value::as_str)
            .or(level)
            .unwrap_or("-")
            .to_string(),
        // Anthropic-провайдеру уровень уходит в том же поле — с потолком и по умолчанию.
        ProviderKind::Anthropic => level.unwrap_or("-").to_string(),
    };
    info!(
        provider = %provider.id,
        model = %upstream_model,
        stream,
        effort = %asked,
        reasoning_effort = sent,
        "POST /v1/messages"
    );

    let session = crate::state::session_id(&headers, &body);
    let primary = attempt(
        &state,
        &provider,
        &route,
        &response_model,
        &body,
        stream,
        started,
        session.as_deref(),
        &uri,
        &headers,
    )
    .await;
    let failure = match primary {
        Ok(response) => return response,
        Err(failure) => failure,
    };
    // Запасная — одна попытка, пока клиенту не отправлен ни байт ответа (ADR-023).
    let backup = state
        .router
        .backup
        .clone()
        .filter(|backup| backup.provider != route.provider || backup.model != route.model);
    let Some(backup) = backup.filter(|_| failure.retryable()) else {
        return failure.response(&provider);
    };
    let backup_provider = &state.router.providers[backup.provider];
    info!(
        provider = %provider.id,
        model = %upstream_model,
        backup_provider = %backup_provider.id,
        backup_model = %backup.model,
        reason = %failure.label(),
        "no usable answer from the model, trying the backup"
    );
    match attempt(
        &state,
        backup_provider,
        &backup,
        &response_model,
        &body,
        stream,
        started,
        session.as_deref(),
        &uri,
        &headers,
    )
    .await
    {
        Ok(response) => response,
        Err(second) => {
            warn!(
                provider = %provider.id,
                model = %upstream_model,
                backup_provider = %backup_provider.id,
                backup_model = %backup.model,
                first = %failure.label(),
                second = %second.label(),
                "both the model and the backup gave no usable answer"
            );
            failure.response(&provider)
        }
    }
}

/// Один заход к провайдеру. Тело — исходное клиентское: модель и уровень рассуждения —
/// параметры попытки, поэтому запрос к запасной строится по её протоколу и потолку (ADR-023).
#[allow(clippy::too_many_arguments)]
async fn attempt(
    state: &AppState,
    provider: &Provider,
    route: &Route,
    client_model: &str,
    body: &Value,
    stream: bool,
    started: Instant,
    session: Option<&str>,
    uri: &Uri,
    incoming: &HeaderMap,
) -> Result<Response, Failure> {
    let mut body = body.clone();
    body["model"] = json!(route.model);
    if provider.kind == ProviderKind::Anthropic
        && let Some(level) = provider.reasoning_effort(&body)
    {
        if !body.get("output_config").is_some_and(Value::is_object) {
            body["output_config"] = json!({});
        }
        body["output_config"]["effort"] = json!(level);
    }
    match provider.kind {
        ProviderKind::Anthropic => {
            passthrough::messages(state, provider, uri, incoming, body, stream, started).await
        }
        ProviderKind::OpenAi => {
            openai::messages(
                state,
                provider,
                client_model,
                &route.model,
                body,
                stream,
                started,
                session,
            )
            .await
        }
    }
}

pub async fn count_tokens(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !auth::authorized(&headers, &state.token, state.open_mode) {
        return error::anthropic_error(401, "authentication_error", AUTH_ERROR);
    }
    let mut body: Value = match serde_json::from_slice(&body) {
        Ok(body) => body,
        Err(_) => {
            return error::anthropic_error(400, "invalid_request_error", "body is not JSON");
        }
    };
    let requested = body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let Some(route) = state.router.resolve(&requested) else {
        return error::anthropic_error(503, "api_error", "no models configured");
    };
    let provider = state.router.providers[route.provider].clone();
    let upstream_model = route.model.clone();
    body["model"] = json!(upstream_model);

    if provider.kind == ProviderKind::Anthropic {
        match passthrough::count_tokens(&state, &provider, &headers, &body).await {
            Ok(response) => return response,
            Err(exc) => {
                warn!(provider = %provider.id, error = %exc, "count_tokens: local estimate");
            }
        }
    }
    estimate_reply(&body).await
}

pub async fn oauth_authorize(body: Bytes) -> Response {
    let body: Value = serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
    let redirect = body
        .get("redirect_uri")
        .and_then(Value::as_str)
        .unwrap_or("http://localhost:4000/desktop/callback")
        .trim()
        .to_string();
    let code = Uuid::new_v4().simple().to_string();
    let separator = if redirect.contains('?') { '&' } else { '?' };
    let mut url = format!("{redirect}{separator}code={code}");
    if let Some(state) = body.get("state").filter(|value| !value.is_null()) {
        let state = state
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| state.to_string());
        url.push_str("&state=");
        url.push_str(&state);
    }
    info!("POST /v1/oauth/authorize -> code issued");
    (StatusCode::OK, Json(json!({"redirect_uri": url}))).into_response()
}

pub async fn oauth_token(State(state): State<AppState>, body: Bytes) -> Response {
    let body: Value = serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
    let scope = body
        .get("scope")
        .and_then(Value::as_str)
        .filter(|scope| !scope.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| state.oauth_scope.clone());
    let grant = body
        .get("grant_type")
        .and_then(Value::as_str)
        .unwrap_or("authorization_code");
    info!(grant, "POST /v1/oauth/token -> token issued");
    (
        StatusCode::OK,
        Json(json!({
            "access_token": format!("sk-ant-oat-local-{}", Uuid::new_v4().simple()),
            "refresh_token": format!("sk-ant-ort-local-{}", Uuid::new_v4().simple()),
            "expires_in": 31536000,
            "scope": scope
        })),
    )
        .into_response()
}

pub async fn api_stub(method: Method, uri: Uri) -> Response {
    info!("{} {} -> 200 (api stub)", method, uri.path());
    (StatusCode::OK, Json(json!({}))).into_response()
}

pub async fn not_found(uri: Uri) -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({"error": "not found", "path": uri.path()})),
    )
        .into_response()
}

async fn estimate_reply(body: &Value) -> Response {
    let text = json!([body.get("system"), body.get("messages"), body.get("tools")]);
    // Подсчёт длинного разговора — десятки миллисекунд процессора (замер: ~55 мс на 3 МБ
    // с картинками, первый вызов ~250 мс): не на рабочем потоке async-рантайма.
    let tokens =
        match tokio::task::spawn_blocking(move || ctp_core::tokens::count_json_tokens(&text)).await
        {
            Ok(tokens) => tokens,
            // Заниженное число помешало бы Claude Code вовремя сжать контекст: лучше отказ.
            Err(exc) => return error::anthropic_error(500, "api_error", exc.to_string()),
        };
    (StatusCode::OK, Json(json!({"input_tokens": tokens}))).into_response()
}

fn summarize_content(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    }
}

fn truncate_chars(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

/// Без советника: объявление `advisor_*` из инструментов и его блоки из истории.
fn strip_advisor(body: &mut Value) {
    if let Some(tools) = body.get_mut("tools").and_then(Value::as_array_mut) {
        let before = tools.len();
        tools.retain(|tool| {
            !tool
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|kind| kind.starts_with("advisor_"))
        });
        // Был только советник: пустой `tools` OpenAI отвергает (400), а выбор инструмента
        // без инструментов не нужен.
        if tools.is_empty()
            && before > 0
            && let Some(fields) = body.as_object_mut()
        {
            for key in ["tools", "tool_choice"] {
                fields.remove(key);
            }
        }
    }
    for message in body
        .get_mut("messages")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        if let Some(content) = message.get_mut("content").and_then(Value::as_array_mut) {
            let before = content.len();
            content.retain(|block| {
                let kind = block.get("type").and_then(Value::as_str);
                !(kind == Some("advisor_tool_result")
                    || kind == Some("server_tool_use")
                        && block.get("name").and_then(Value::as_str) == Some("advisor"))
            });
            // Пустое содержимое Anthropic отвергает (400).
            if content.is_empty() && before > 0 {
                content.push(json!({"type": "text", "text": "…"}));
            }
        }
    }
}

fn is_probe(body: &Value) -> bool {
    let max_tokens = body
        .get("max_tokens")
        .and_then(Value::as_f64)
        .is_some_and(|value| (value - 1.0).abs() < f64::EPSILON);
    if !max_tokens {
        return false;
    }
    if body
        .get("tools")
        .and_then(Value::as_array)
        .is_some_and(|tools| !tools.is_empty())
    {
        return false;
    }
    let Some(messages) = body.get("messages").and_then(Value::as_array) else {
        return false;
    };
    if messages.len() != 1 {
        return false;
    }
    let message = &messages[0];
    if message.get("role").and_then(Value::as_str) != Some("user") {
        return false;
    }
    let text = match message.get("content") {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(""),
        _ => return false,
    };
    matches!(text.trim(), "." | "")
}

fn probe_reply(model: &str, stream: bool) -> Response {
    let message = json!({
        "id": "msg_probe",
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": [{"type": "text", "text": "."}],
        "stop_reason": "max_tokens",
        "stop_sequence": null,
        "usage": {"input_tokens": 1, "output_tokens": 1}
    });
    if !stream {
        return (StatusCode::OK, Json(message)).into_response();
    }

    let start = json!({
        "id": "msg_probe",
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": [],
        "stop_reason": null,
        "stop_sequence": null,
        "usage": {"input_tokens": 1, "output_tokens": 0}
    });
    sse::response(vec![
        json!({"type": "message_start", "message": start}),
        json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "."}}),
        json!({"type": "content_block_stop", "index": 0}),
        json!({"type": "message_delta", "delta": {"stop_reason": "max_tokens"}, "usage": {"output_tokens": 1}}),
        json!({"type": "message_stop"}),
    ])
}
