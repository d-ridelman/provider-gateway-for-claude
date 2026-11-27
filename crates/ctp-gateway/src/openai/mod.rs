use crate::error;
use crate::passthrough::RequestLog;
use crate::sse;
use crate::state::{AppState, Provider};
use crate::upstream::Failure;
use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::convert::Infallible;
use std::time::Instant;
use tracing::{info, warn};

pub mod adapt;
pub mod request;
pub mod response;
pub mod stream;

/// Запрос к провайдеру. Один повтор, если он ругается на слишком большой max_tokens.
/// Неясный 400 — лесенка упрощений (`adapt`): шаги по одному, пока не примет; помогало
/// дважды — модель выучена и запрос упрощается заранее.
async fn send(
    state: &AppState,
    provider: &Provider,
    headers: &HeaderMap,
    upstream_model: &str,
    payload: &mut Value,
) -> Result<reqwest::Response, Failure> {
    let started = Instant::now();
    let url = provider.chat_url();
    let key = format!("{}/{}", provider.id, upstream_model);
    let known = state.profiles.steps(&key);
    for step in &known {
        adapt::apply(*step, payload, provider);
    }
    let mut attempt = 0u8;
    // Шаги, добавленные в этом вызове после отказов, и следующая ступень лесенки.
    let mut applied: Vec<adapt::Step> = Vec::new();
    let mut rung = 0usize;
    // Картинки запроса: на успехе без упрощений — признак, что они модели не мешают.
    let had_images = request::has_images(payload);
    // Отпечатки убранных картинок и первый отказ (его текст и вернём, если не поможет).
    let mut removed_images: Vec<u64> = Vec::new();
    // Вынутое ступенью «без рассуждения» — вернуть, если она не помогла.
    let mut reasoning_taken: Option<Vec<(usize, Value)>> = None;
    // Ступень «без рассуждения» уже пробовалась (одна) и пробовалась ли последней (поверх всего).
    let mut reasoning_tried = false;
    let mut reasoning_last = false;
    let mut first_refusal: Option<(u16, String)> = None;
    'send: loop {
        let request = state
            .clients
            .stream
            .post(&url)
            .headers(headers.clone())
            .json(&*payload);
        let response = match request.send().await {
            Ok(response) => response,
            Err(exc) => {
                warn!(provider = %provider.id, error = %exc, "OpenAI upstream: network");
                return Err(Failure::Network(exc.to_string()));
            }
        };
        if response.status().is_success() {
            if let Some(flipped) = applied.last().copied() {
                info!(
                    provider = %provider.id,
                    model = %upstream_model,
                    steps = %labels(&applied),
                    "the request went through after simplifying it"
                );
                let images: &[u64] = if flipped == adapt::Step::Images {
                    &removed_images
                } else {
                    &[]
                };
                if let Some(learned) = state
                    .profiles
                    .record_success_since(&key, flipped, images, started)
                {
                    info!(
                        provider = %provider.id,
                        model = %upstream_model,
                        steps = %labels(&learned),
                        "learned: from now on the requests to this model are simplified up front"
                    );
                }
            } else if had_images && !known.contains(&adapt::Step::Images) {
                state.profiles.images_are_fine(&key);
            }
            return Ok(response);
        }

        let mut status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let mut text = response.text().await.unwrap_or_default();
        if attempt == 0
            && let Some(limit) = crate::util::parse_output_limit(&adapt::error_message(&text))
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
        let reason = error::clean_error_text(&text, &provider.api_key);
        if adapt::adaptable(status, &text) && !state.profiles.cooling(&key) {
            // Запрос без рассуждения всё равно не прошёл: поле нужно этому провайдеру (DeepSeek),
            // а отказ — по другой причине. Поле возвращается, иначе остальные ступени пробовались
            // бы на запросе, который сломан заведомо.
            if let Some(saved) = reasoning_taken.take() {
                request::restore_reasoning(payload, saved);
                applied.retain(|step| *step != adapt::Step::Reasoning);
            }
            while rung < adapt::LADDER.len() {
                let step = adapt::LADDER[rung];
                rung += 1;
                if known.contains(&step) {
                    continue;
                }
                if step == adapt::Step::Images {
                    removed_images = adapt::image_ids(payload);
                }
                let changed = if step == adapt::Step::Reasoning {
                    let saved = request::take_reasoning(payload);
                    let changed = !saved.is_empty();
                    reasoning_tried |= changed;
                    reasoning_taken = changed.then_some(saved);
                    changed
                } else {
                    adapt::apply(step, payload, provider)
                };
                if changed {
                    first_refusal.get_or_insert((status, text.clone()));
                    info!(
                        provider = %provider.id,
                        model = %upstream_model,
                        status,
                        step = step.label(),
                        provider_said = %snippet(&reason),
                        "the provider refused the request, retrying simplified"
                    );
                    applied.push(step);
                    continue 'send;
                }
            }
            // Лесенка кончилась, а поле на месте: убираем и его, поверх уже применённых ступеней, —
            // провайдеру могут мешать и оно, и что-то ещё (картинка, лишние инструменты).
            if reasoning_tried && !reasoning_last && !applied.is_empty() {
                let saved = request::take_reasoning(payload);
                if !saved.is_empty() {
                    reasoning_last = true;
                    reasoning_taken = Some(saved);
                    applied.push(adapt::Step::Reasoning);
                    info!(
                        provider = %provider.id,
                        model = %upstream_model,
                        status,
                        step = adapt::Step::Reasoning.label(),
                        provider_said = %snippet(&reason),
                        "the provider refused the request, retrying with the reasoning left out as well"
                    );
                    continue 'send;
                }
            }
            // Ничего не помогло: это не капризы модели, а сам запрос. Клиенту и в журнал —
            // отказ на исходный запрос, а не на упрощённый до неузнаваемости.
            if !applied.is_empty() || reasoning_tried {
                state.profiles.cool(&key);
            }
            if let Some((first_status, first_text)) = first_refusal {
                status = first_status;
                text = first_text;
            }
        }
        let reason = error::clean_error_text(&text, &provider.api_key);
        warn!(
            provider = %provider.id,
            model = %upstream_model,
            status,
            provider_said = %snippet(&reason),
            "OpenAI upstream error"
        );
        return Err(Failure::Status {
            status,
            text,
            retry_after,
            raw: None,
        });
    }
}

fn labels(steps: &[adapt::Step]) -> String {
    steps
        .iter()
        .map(|step| step.label())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Начало ответа провайдера для строки журнала.
fn snippet(text: &str) -> String {
    let text: String = text.chars().take(240).collect();
    text.replace(['\n', '\r'], " ")
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn messages(
    state: &AppState,
    provider: &Provider,
    client_model: &str,
    upstream_model: &str,
    body: Value,
    stream: bool,
    started: Instant,
    session: Option<&str>,
) -> Result<Response, Failure> {
    let mut payload = request::build(provider, &body, stream);
    // Если провайдеру придётся сокращать имена инструментов, в ответе они вернутся прежними.
    let names = adapt::name_map(&body);
    let headers = upstream_headers(provider, session);
    // Подсчёт длинного разговора — десятки миллисекунд процессора (замер: ~55 мс на 3 МБ с
    // картинками, первый вызов ~250 мс): не на рабочем потоке, пока рядом идут потоки субагентов.
    let counted =
        tokio::task::spawn_blocking(move || ctp_core::tokens::count_json_tokens(&body)).await;
    let input_tokens = match counted {
        Ok(tokens) => tokens,
        Err(exc) => return Ok(error::anthropic_error(500, "api_error", exc.to_string())),
    };

    let upstream = send(state, provider, &headers, upstream_model, &mut payload).await?;

    let status = upstream.status();
    if !stream {
        let text = match upstream.text().await {
            Ok(text) => text,
            Err(exc) => return Err(Failure::Body(exc.to_string())),
        };
        let value: Value = match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(_) => return Err(Failure::Body("provider returned non-JSON".into())),
        };
        if value.get("error").is_some() {
            return Err(Failure::Status {
                status: status.as_u16(),
                text,
                retry_after: None,
                raw: None,
            });
        }
        let mut message = match response::translate(&value, client_model, input_tokens) {
            Ok(message) => message,
            Err(exc) => return Err(Failure::Body(exc.to_string())),
        };
        response::restore_names(&mut message, &names);
        info!(
            provider = %provider.id,
            model = %upstream_model,
            ms = started.elapsed().as_millis(),
            "POST /v1/messages openai"
        );
        return Ok((StatusCode::OK, axum::Json(message)).into_response());
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
            Err(exc) => return Err(Failure::Body(exc.to_string())),
        };
        let value: Value = match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(_) => return Err(Failure::Body("provider returned non-JSON".into())),
        };
        if value.get("error").is_some() {
            return Err(Failure::Status {
                status: status.as_u16(),
                text,
                retry_after: None,
                raw: None,
            });
        }
        let mut message = match response::translate(&value, client_model, input_tokens) {
            Ok(message) => message,
            Err(exc) => return Err(Failure::Body(exc.to_string())),
        };
        response::restore_names(&mut message, &names);
        info!(
            provider = %provider.id,
            model = %upstream_model,
            "POST /v1/messages openai: stream replaced with a whole response"
        );
        return Ok(sse::response(response::to_events(&message)));
    }

    let mut translator = stream::StreamTranslator::new(client_model, input_tokens)
        .with_secret(provider.api_key.clone())
        .with_names(names);
    let start = translator.start();
    let log = RequestLog::new(started, provider.id.clone(), "openai stream");
    let events = async_stream::stream! {
        let _log = log;
        yield Ok::<Bytes, Infallible>(sse::frame(&start));

        let mut ticker = tokio::time::interval_at(
            tokio::time::Instant::now() + std::time::Duration::from_secs(15),
            std::time::Duration::from_secs(15),
        );
        let mut source = upstream.bytes_stream().eventsource();
        let mut saw_done = false;
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
                    translator.abort();
                    break;
                }
            };
            if event.data.trim() == "[DONE]" {
                saw_done = true;
                break;
            }
            let chunk: Value = match serde_json::from_str(&event.data) {
                Ok(chunk) => chunk,
                Err(_) => {
                    yield Ok::<Bytes, Infallible>(sse::frame(
                        &stream::StreamTranslator::error_event("upstream sent invalid JSON")
                    ));
                    translator.abort();
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

        // Поток кончился без `[DONE]` и без `finish_reason` — провайдер оборвал ответ. Это
        // ошибка, а не `end_turn`: иначе Claude Code примет обрезанный ответ за полный.
        if !translator.is_done() && !saw_done && !translator.has_finish_reason() {
            yield Ok::<Bytes, Infallible>(sse::frame(&stream::StreamTranslator::error_event(
                "upstream stream ended before the answer was finished",
            )));
            translator.abort();
        }
        if !translator.is_done() {
            for value in translator.finish() {
                yield Ok::<Bytes, Infallible>(sse::frame(&value));
            }
        }
    };

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from_stream(events))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()))
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
