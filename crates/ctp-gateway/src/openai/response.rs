use ctp_core::tokens::count_json_tokens;
use serde_json::{Value, json};
use uuid::Uuid;

/// OpenAI Chat Completions (ответ) → Anthropic Messages (ответ).
pub fn translate(
    value: &Value,
    client_model: &str,
    fallback_input_tokens: u64,
) -> anyhow::Result<Value> {
    let choice = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .ok_or_else(|| anyhow::anyhow!("provider response has no choices"))?;
    let message = choice.get("message").unwrap_or(&Value::Null);

    let mut content = Vec::new();
    let reasoning = reasoning_text(message);
    if !reasoning.is_empty() {
        content.push(json!({"type": "thinking", "thinking": reasoning}));
    }
    let text = content_text(message.get("content"));
    if !text.is_empty() {
        content.push(json!({"type": "text", "text": text}));
    }

    let tool_calls = message
        .get("tool_calls")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for call in &tool_calls {
        let name = call
            .get("function")
            .and_then(|function| function.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if name.is_empty() {
            continue;
        }
        let id = call
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("toolu_{}", Uuid::new_v4().simple()));
        let input = tool_input(
            call.get("function")
                .and_then(|function| function.get("arguments")),
        );
        content.push(json!({"type": "tool_use", "id": id, "name": name, "input": input}));
    }

    // По блокам, которые ушли Claude Code: вызов без имени отброшен выше.
    let has_tools = content
        .iter()
        .any(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"));
    let finish = choice.get("finish_reason").and_then(Value::as_str);
    let stop_reason = map_stop_reason(finish, has_tools);

    let fallback_output = if content.is_empty() {
        0
    } else {
        count_json_tokens(&Value::Array(content.clone()))
    };
    let usage = usage_tokens(value, fallback_input_tokens, fallback_output);
    let mut usage_json = json!({
        "input_tokens": usage.input,
        "output_tokens": usage.output
    });
    if let Some(cached) = usage.cached {
        usage_json["cache_read_input_tokens"] = json!(cached);
    }

    Ok(json!({
        "id": format!("msg_{}", Uuid::new_v4().simple()),
        "type": "message",
        "role": "assistant",
        "model": client_model,
        "content": content,
        "stop_reason": stop_reason,
        "stop_sequence": Value::Null,
        "usage": usage_json
    }))
}

pub fn to_events(message: &Value) -> Vec<Value> {
    let mut events = Vec::new();
    let mut start = message.clone();
    start["content"] = json!([]);
    start["stop_reason"] = Value::Null;
    start["stop_sequence"] = Value::Null;
    start["usage"]["output_tokens"] = json!(0);
    events.push(json!({"type": "message_start", "message": start}));

    let mut index = 0u32;
    if let Some(blocks) = message.get("content").and_then(Value::as_array) {
        for block in blocks {
            match block.get("type").and_then(Value::as_str).unwrap_or("") {
                "text" => {
                    let text = block.get("text").and_then(Value::as_str).unwrap_or("");
                    events.push(json!({
                        "type": "content_block_start",
                        "index": index,
                        "content_block": {"type": "text", "text": ""}
                    }));
                    if !text.is_empty() {
                        events.push(json!({
                            "type": "content_block_delta",
                            "index": index,
                            "delta": {"type": "text_delta", "text": text}
                        }));
                    }
                    events.push(json!({"type": "content_block_stop", "index": index}));
                    index += 1;
                }
                "thinking" => {
                    let thinking = block.get("thinking").and_then(Value::as_str).unwrap_or("");
                    events.push(json!({
                        "type": "content_block_start",
                        "index": index,
                        "content_block": {"type": "thinking", "thinking": ""}
                    }));
                    if !thinking.is_empty() {
                        events.push(json!({
                            "type": "content_block_delta",
                            "index": index,
                            "delta": {"type": "thinking_delta", "thinking": thinking}
                        }));
                    }
                    if let Some(signature) = block.get("signature").and_then(Value::as_str)
                        && !signature.is_empty()
                    {
                        events.push(json!({
                            "type": "content_block_delta",
                            "index": index,
                            "delta": {"type": "signature_delta", "signature": signature}
                        }));
                    }
                    events.push(json!({"type": "content_block_stop", "index": index}));
                    index += 1;
                }
                "tool_use" => {
                    let id = block.get("id").and_then(Value::as_str).unwrap_or("");
                    let name = block.get("name").and_then(Value::as_str).unwrap_or("");
                    let input = block.get("input").cloned().unwrap_or_else(|| json!({}));
                    let partial = serde_json::to_string(&input).unwrap_or_else(|_| "{}".into());
                    events.push(json!({
                        "type": "content_block_start",
                        "index": index,
                        "content_block": {"type": "tool_use", "id": id, "name": name, "input": {}}
                    }));
                    events.push(json!({
                        "type": "content_block_delta",
                        "index": index,
                        "delta": {"type": "input_json_delta", "partial_json": partial}
                    }));
                    events.push(json!({"type": "content_block_stop", "index": index}));
                    index += 1;
                }
                _ => {}
            }
        }
    }

    events.push(json!({
        "type": "message_delta",
        "delta": {
            "stop_reason": message.get("stop_reason").cloned().unwrap_or(Value::Null),
            "stop_sequence": message.get("stop_sequence").cloned().unwrap_or(Value::Null)
        },
        "usage": {"output_tokens": message.get("usage").and_then(|u| u.get("output_tokens")).cloned().unwrap_or(json!(0))}
    }));
    events.push(json!({"type": "message_stop"}));
    events
}

pub fn content_text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|part| {
                part.get("text")
                    .and_then(Value::as_str)
                    .or_else(|| part.get("content").and_then(Value::as_str))
                    .map(str::to_string)
            })
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

pub fn reasoning_text(message: &Value) -> String {
    for key in ["reasoning_content", "reasoning"] {
        if let Some(text) = message.get(key).and_then(Value::as_str)
            && !text.is_empty()
        {
            return text.to_string();
        }
    }
    if let Some(details) = message.get("reasoning_details").and_then(Value::as_array) {
        return details
            .iter()
            .filter_map(|detail| detail.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("");
    }
    String::new()
}

pub fn map_stop_reason(finish: Option<&str>, has_tools: bool) -> &'static str {
    if has_tools {
        return "tool_use";
    }
    // `tool_calls` без единого вызова (советник исполнен шлюзом, вызов без имени
    // отброшен) — не `tool_use`: Claude Code ждал бы инструмент, которого нет.
    match finish {
        Some("length") => "max_tokens",
        Some("content_filter") => "refusal",
        _ => "end_turn",
    }
}

fn tool_input(arguments: Option<&Value>) -> Value {
    match arguments {
        Some(Value::String(text)) => match serde_json::from_str::<Value>(text) {
            Ok(Value::Object(map)) => Value::Object(map),
            _ => json!({}),
        },
        Some(Value::Object(map)) => Value::Object(map.clone()),
        _ => json!({}),
    }
}

struct Usage {
    input: u64,
    output: u64,
    cached: Option<u64>,
}

fn usage_tokens(value: &Value, fallback_input: u64, fallback_output: u64) -> Usage {
    let usage = value.get("usage");
    let (input, cached) = usage
        .and_then(input_usage)
        .unwrap_or((fallback_input, None));
    let output = usage
        .and_then(|usage| {
            usage
                .get("completion_tokens")
                .or_else(|| usage.get("output_tokens"))
        })
        .and_then(Value::as_u64)
        .unwrap_or(fallback_output);
    Usage {
        input,
        output,
        cached,
    }
}

/// Входные токены провайдера в смысле Anthropic: `input_tokens` без кэша и отдельно
/// `cache_read_input_tokens`. У OpenAI `prompt_tokens` кэш уже включает — иначе Claude Code
/// сложил бы его дважды и счёл контекст больше настоящего.
/// Ноль входных — не цифра провайдера, а её отсутствие (так шлют некоторые ретрансляторы):
/// Claude Code счёл бы контекст пустым и не сжал бы его до переполнения окна.
pub fn input_usage(usage: &Value) -> Option<(u64, Option<u64>)> {
    let positive = |value: Option<&Value>| value.and_then(Value::as_u64).filter(|n| *n > 0);
    if let Some(prompt) = positive(usage.get("prompt_tokens")) {
        // OpenAI: кэш — часть prompt_tokens.
        let cached = positive(
            usage
                .get("prompt_tokens_details")
                .and_then(|details| details.get("cached_tokens")),
        )
        .map(|cached| cached.min(prompt));
        return Some((prompt - cached.unwrap_or(0), cached));
    }
    // Anthropic: input_tokens кэш уже не включает.
    let input = usage
        .get("input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cached = positive(usage.get("cache_read_input_tokens"));
    (input > 0 || cached.is_some()).then_some((input, cached))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_calls_finish_without_a_named_call_ends_the_turn() {
        let value = json!({
            "choices": [{"index": 0, "finish_reason": "tool_calls", "message": {
                "role": "assistant", "content": "hi",
                "tool_calls": [{"id": "c1", "type": "function", "function": {"name": "", "arguments": "{}"}}]
            }}]
        });
        let message = translate(&value, "m", 1).unwrap();
        assert_eq!(message["stop_reason"], "end_turn");
        assert_eq!(map_stop_reason(Some("tool_calls"), false), "end_turn");
        assert_eq!(map_stop_reason(Some("tool_calls"), true), "tool_use");
    }

    #[test]
    fn zero_provider_input_keeps_the_estimate() {
        let value = json!({
            "choices": [{"index": 0, "finish_reason": "stop", "message": {"role": "assistant", "content": "hi"}}],
            "usage": {"prompt_tokens": 0, "completion_tokens": 1}
        });
        let message = translate(&value, "m", 4321).unwrap();
        assert_eq!(message["usage"]["input_tokens"], 4321);
    }

    #[test]
    fn translates_text_and_tool_call() {
        let value = json!({
            "id": "chatcmpl-1",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "hello",
                    "tool_calls": [{
                        "id": "call_1", "type": "function",
                        "function": {"name": "read", "arguments": "{\"path\":\"a.txt\"}"}
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5}
        });
        let out = translate(&value, "claude-openai-80673633", 1).unwrap();
        assert_eq!(out["content"][0]["type"], "text");
        assert_eq!(out["content"][1]["type"], "tool_use");
        assert_eq!(out["content"][1]["input"]["path"], "a.txt");
        assert_eq!(out["stop_reason"], "tool_use");
        assert_eq!(out["usage"]["input_tokens"], 10);
        assert_eq!(out["usage"]["output_tokens"], 5);
    }

    #[test]
    fn synthesizes_sse_in_the_right_order() {
        let message = json!({
            "id": "msg_1", "type": "message", "role": "assistant",
            "model": "claude-x", "content": [
                {"type": "text", "text": "hi"},
                {"type": "tool_use", "id": "toolu_1", "name": "read", "input": {"path": "a"}}
            ],
            "stop_reason": "tool_use", "stop_sequence": null,
            "usage": {"input_tokens": 3, "output_tokens": 2}
        });
        let events = to_events(&message);
        let types: Vec<&str> = events
            .iter()
            .map(|event| event["type"].as_str().unwrap())
            .collect();
        assert_eq!(
            types,
            vec![
                "message_start",
                "content_block_start",
                "content_block_delta",
                "content_block_stop",
                "content_block_start",
                "content_block_delta",
                "content_block_stop",
                "message_delta",
                "message_stop"
            ]
        );
    }
}
