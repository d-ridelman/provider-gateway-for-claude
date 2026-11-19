//! Советник (`advisor` Claude Code) силами шлюза. У Anthropic это серверный инструмент:
//! модель вызывает его посреди ответа, сервер даёт сильной модели весь разговор и
//! вставляет её совет в тот же ответ. OpenAI-провайдер так не умеет — это делает шлюз.

use crate::error;
use crate::openai::{response, upstream_headers};
use crate::state::{AppState, ProviderKind, Route};
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use tracing::{info, warn};

/// Вызовов советника на один ответ модели; дальше функция из продолжения убирается.
pub const MAX_CALLS: u32 = 3;

const TIMEOUT: Duration = Duration::from_secs(600);
const MAX_OUTPUT_TOKENS: u64 = 8192;
/// Бюджет расшифровки разговора, в символах (~150K токенов): старое выпадает первым.
const BUDGET: usize = 600_000;
const SYSTEM_LIMIT: usize = 60_000;
const BLOCK_LIMIT: usize = 12_000;

const SYSTEM: &str = "You are the advisor: a stronger model that an AI coding agent (the \
executor) consults in the middle of its work. Below is the executor's conversation so far: \
its instructions, the user's requests, its own messages, tool calls and tool results. The \
executor has just called you. Review the conversation and reply to the executor with concise, \
concrete guidance: whether its approach is sound, what is wrong, risky or missing, a shorter \
or safer path, and what to verify before declaring the task done. Do not write the whole \
solution and do not call tools. Write in the language the user writes in. Keep it short \
unless more is essential.";

/// Совет сильной модели на разговор `messages` (OpenAI-сообщения, как у основного запроса).
/// Ошибка — код для `advisor_tool_result_error`, понятный Claude Code.
pub async fn consult(
    state: &AppState,
    route: &Route,
    messages: &[Value],
    session: Option<&str>,
) -> Result<String, &'static str> {
    let provider = &state.router.providers[route.provider];
    if provider.kind == ProviderKind::Anthropic {
        // shortcut: советник только на OpenAI-совместимом провайдере; Anthropic-тип — сделать, если кто-то выберет такую сильную модель.
        warn!(provider = %provider.id, "advisor on an Anthropic-type provider is not supported");
        return Err("unavailable");
    }
    let started = Instant::now();
    let mut body = json!({
        "model": route.model,
        "messages": [
            {"role": "system", "content": SYSTEM},
            {"role": "user", "content": transcript(messages)}
        ]
    });
    if let Some(key) = provider.max_tokens_key(&route.model) {
        body[key.as_str()] = json!(MAX_OUTPUT_TOKENS);
    }
    if let Some(effort) = provider.reasoning_effort(&Value::Null) {
        body["reasoning_effort"] = json!(effort);
    }
    for (key, value) in &provider.extra_body {
        body[key.as_str()] = value.clone();
    }

    // Потоком: целый ответ на большой разговор сильная модель готовит минутами, и
    // посредники по дороге рвут соединение, в котором долго нет ни байта.
    body["stream"] = json!(true);
    if provider.include_usage {
        body["stream_options"] = json!({"include_usage": true});
    }
    let headers = upstream_headers(provider, session);
    let deadline = tokio::time::Instant::now() + TIMEOUT;
    let mut retried = false;
    let advice = loop {
        let request = state
            .clients
            .stream
            .post(provider.chat_url())
            .headers(headers.clone())
            .json(&body)
            .send();
        let response = match tokio::time::timeout_at(deadline, request).await {
            Err(_) => return Err("execution_time_exceeded"),
            Ok(Err(exc)) => {
                warn!(provider = %provider.id, error = %error_chain(&exc), "advisor: network");
                return Err("unavailable");
            }
            Ok(Ok(response)) => response,
        };
        let status = response.status().as_u16();
        if (200..300).contains(&status) {
            match tokio::time::timeout_at(deadline, read_answer(response)).await {
                Err(_) => return Err("execution_time_exceeded"),
                Ok(Err(text)) => {
                    warn!(
                        provider = %provider.id,
                        model = %route.model,
                        error = %error::clean_error_text(&text, &provider.api_key),
                        "advisor: stream failed"
                    );
                    return Err("unavailable");
                }
                Ok(Ok(advice)) => break advice,
            }
        }
        let text = match tokio::time::timeout_at(deadline, response.text()).await {
            Ok(Ok(text)) => text,
            Ok(Err(_)) => return Err("unavailable"),
            Err(_) => return Err("execution_time_exceeded"),
        };
        if !retried
            && let Some(limit) = crate::util::parse_output_limit(&text)
            && let Some(key) = provider.max_tokens_key(&route.model)
        {
            body[key.as_str()] = json!(limit.min(MAX_OUTPUT_TOKENS));
            retried = true;
            continue;
        }
        warn!(
            provider = %provider.id,
            model = %route.model,
            status,
            error = %error::clean_error_text(&text, &provider.api_key),
            "advisor: upstream error"
        );
        return Err(error_code(status, &text));
    };

    if advice.trim().is_empty() {
        warn!(provider = %provider.id, model = %route.model, "advisor: empty answer");
        return Err("unavailable");
    }
    info!(
        provider = %provider.id,
        model = %route.model,
        ms = started.elapsed().as_millis(),
        chars = advice.chars().count(),
        "advisor consulted"
    );
    Ok(advice)
}

/// Текст ответа: поток OpenAI (`delta.content`) или целый JSON, если провайдер `stream`
/// не поддержал. Ошибка — текст для журнала.
async fn read_answer(response: reqwest::Response) -> Result<String, String> {
    use eventsource_stream::Eventsource;
    use futures_util::StreamExt;

    let is_sse = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_none_or(|kind| kind.contains("text/event-stream"));
    if !is_sse {
        let text = response.text().await.map_err(|exc| error_chain(&exc))?;
        let value: Value = serde_json::from_str(&text).map_err(|_| text.clone())?;
        if value.get("error").is_some() {
            return Err(text);
        }
        return Ok(response::content_text(
            value.pointer("/choices/0/message/content"),
        ));
    }
    let mut advice = String::new();
    // Совет полный, только если поток дошёл до конца: `[DONE]` или `finish_reason`.
    // Оборванный на полуслове совет хуже честного «недоступен».
    let mut complete = false;
    let mut source = response.bytes_stream().eventsource();
    while let Some(event) = source.next().await {
        let event = event.map_err(|exc| error_chain(&exc))?;
        if event.data.trim() == "[DONE]" {
            complete = true;
            break;
        }
        let chunk: Value = serde_json::from_str(&event.data).map_err(|_| event.data.clone())?;
        if chunk.get("error").is_some() {
            return Err(event.data);
        }
        advice.push_str(&response::content_text(
            chunk.pointer("/choices/0/delta/content"),
        ));
        complete |= chunk
            .pointer("/choices/0/finish_reason")
            .and_then(Value::as_str)
            .is_some_and(|reason| !reason.is_empty());
    }
    if complete {
        Ok(advice)
    } else {
        Err(format!(
            "stream ended before the answer was complete ({} chars received)",
            advice.chars().count()
        ))
    }
}

/// Ошибка с причинами: у reqwest сам текст — «error sending request», суть в источниках.
fn error_chain(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

/// Код ошибки советника из списка, который знает Claude Code.
fn error_code(status: u16, text: &str) -> &'static str {
    let lower = text.to_ascii_lowercase();
    match status {
        429 => "too_many_requests",
        503 | 529 => "overloaded",
        400 | 413
            if lower.contains("context")
                || lower.contains("too long")
                || lower.contains("too many tokens") =>
        {
            "prompt_too_long"
        }
        _ => "unavailable",
    }
}

/// Разговор текстом: часть провайдеров отвергает `tool_calls` без объявленных инструментов,
/// а советнику инструменты не нужны. Не влезает в бюджет — выпадает начало (кроме
/// системного промпта).
fn transcript(messages: &[Value]) -> String {
    let mut system = String::new();
    let mut parts: Vec<String> = Vec::new();
    for message in messages {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("user");
        let text = response::content_text(message.get("content"));
        match role {
            "system" => system = clip(&text, SYSTEM_LIMIT),
            "tool" => parts.push(format!("## Tool result\n{}", clip(&text, BLOCK_LIMIT))),
            "assistant" => {
                let mut part = String::from("## Executor");
                if !text.trim().is_empty() {
                    part.push('\n');
                    part.push_str(&clip(&text, BLOCK_LIMIT));
                }
                for call in message
                    .get("tool_calls")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let name = call
                        .pointer("/function/name")
                        .and_then(Value::as_str)
                        .unwrap_or("?");
                    let arguments = call
                        .pointer("/function/arguments")
                        .and_then(Value::as_str)
                        .unwrap_or("{}");
                    part.push_str(&format!(
                        "\n→ calls {name} {}",
                        clip(arguments, BLOCK_LIMIT)
                    ));
                }
                parts.push(part);
            }
            _ => parts.push(format!("## User\n{}", clip(&text, BLOCK_LIMIT))),
        }
    }

    let mut kept: Vec<String> = Vec::new();
    let mut used = system.len();
    for part in parts.iter().rev() {
        if used + part.len() > BUDGET && !kept.is_empty() {
            break;
        }
        used += part.len();
        kept.push(part.clone());
    }
    let dropped = parts.len() - kept.len();
    kept.reverse();

    let mut out = String::new();
    if !system.is_empty() {
        out.push_str("# Executor's instructions\n");
        out.push_str(&system);
        out.push_str("\n\n");
    }
    out.push_str("# Conversation\n");
    if dropped > 0 {
        out.push_str(&format!("[{dropped} earlier messages omitted]\n\n"));
    }
    out.push_str(&kept.join("\n\n"));
    out.push_str("\n\n# Your turn\nThe executor called you now. Give your advice.");
    out
}

fn clip(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}… [cut]", &text[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_keeps_roles_calls_and_the_recent_end() {
        let messages = vec![
            json!({"role": "system", "content": "be careful"}),
            json!({"role": "user", "content": "fix the bug"}),
            json!({"role": "assistant", "content": "looking", "tool_calls": [
                {"id": "c1", "type": "function", "function": {"name": "Read", "arguments": "{\"path\":\"a.rs\"}"}}
            ]}),
            json!({"role": "tool", "tool_call_id": "c1", "content": "fn main() {}"}),
        ];
        let text = transcript(&messages);
        assert!(text.starts_with("# Executor's instructions\nbe careful"));
        assert!(text.contains("## User\nfix the bug"));
        assert!(text.contains("→ calls Read {\"path\":\"a.rs\"}"));
        assert!(text.contains("## Tool result\nfn main() {}"));
        assert!(text.ends_with("Give your advice."));

        // Не влезает — выпадает начало, конец остаётся.
        let mut long = vec![json!({"role": "system", "content": "s"})];
        for i in 0..200 {
            long.push(json!({"role": "user", "content": format!("{i} {}", "x".repeat(10_000))}));
        }
        let text = transcript(&long);
        assert!(text.contains("earlier messages omitted"));
        assert!(text.contains("199 x"));
        assert!(!text.contains("## User\n0 x"));
        assert!(text.len() < BUDGET + 2_000);
    }

    #[test]
    fn errors_map_to_codes_claude_code_knows() {
        assert_eq!(error_code(429, ""), "too_many_requests");
        assert_eq!(error_code(503, "busy"), "overloaded");
        assert_eq!(
            error_code(400, "maximum context length exceeded"),
            "prompt_too_long"
        );
        assert_eq!(error_code(400, "bad request"), "unavailable");
        assert_eq!(clip("привет", 3), "п… [cut]");
    }
}
