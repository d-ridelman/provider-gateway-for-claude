use crate::state::Provider;
use serde_json::{Map, Value, json};
use uuid::Uuid;

/// Anthropic Messages → OpenAI Chat Completions (тело запроса).
pub fn build(provider: &Provider, body: &Value, stream: bool) -> Value {
    let model = body.get("model").and_then(Value::as_str).unwrap_or("");
    let mut request = Map::new();
    request.insert("model".into(), json!(model));
    request.insert("messages".into(), Value::Array(convert_messages(body)));

    if let Some(max_tokens) = body.get("max_tokens").and_then(Value::as_i64)
        && let Some(key) = provider.max_tokens_key(model)
    {
        request.insert(key, json!(max_tokens.max(1)));
    }
    for name in ["temperature", "top_p"] {
        if let Some(value) = body.get(name) {
            request.insert(name.into(), value.clone());
        }
    }
    if let Some(stop) = body.get("stop_sequences").and_then(Value::as_array)
        && !stop.is_empty()
    {
        request.insert("stop".into(), Value::Array(stop.clone()));
    }

    let tools = convert_tools(body);
    let has_tools = !tools.is_empty();
    if has_tools {
        request.insert("tools".into(), Value::Array(tools));
    }
    let (choice, parallel) = convert_tool_choice(body.get("tool_choice"), has_tools);
    if let Some(choice) = choice {
        request.insert("tool_choice".into(), choice);
    }
    if let Some(parallel) = parallel {
        request.insert("parallel_tool_calls".into(), json!(parallel));
    }

    // Уровень рассуждения Claude → `reasoning_effort` (только если провайдеру он включён).
    if let Some(effort) = provider.reasoning_effort(body) {
        request.insert("reasoning_effort".into(), json!(effort));
    }

    if stream {
        request.insert("stream".into(), json!(true));
        if provider.include_usage {
            request.insert("stream_options".into(), json!({"include_usage": true}));
        }
    }

    for (key, value) in &provider.extra_body {
        request.insert(key.clone(), value.clone());
    }

    Value::Object(request)
}

fn convert_messages(body: &Value) -> Vec<Value> {
    let mut out = Vec::new();
    let system = system_text(body.get("system").unwrap_or(&Value::Null));
    if !system.trim().is_empty() {
        out.push(json!({"role": "system", "content": system}));
    }

    if let Some(messages) = body.get("messages").and_then(Value::as_array) {
        for message in messages {
            match message
                .get("role")
                .and_then(Value::as_str)
                .unwrap_or("user")
            {
                "assistant" => convert_assistant(message, &mut out),
                _ => convert_user(message, &mut out),
            }
        }
    }
    out
}

fn system_text(system: &Value) -> String {
    match system {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n\n"),
        _ => String::new(),
    }
}

fn convert_user(message: &Value, out: &mut Vec<Value>) {
    let Some(content) = message.get("content") else {
        return;
    };
    if let Value::String(text) = content {
        if !text.is_empty() {
            out.push(json!({"role": "user", "content": text}));
        }
        return;
    }
    let Some(blocks) = content.as_array() else {
        return;
    };

    let mut text_parts: Vec<String> = Vec::new();
    let mut images: Vec<Value> = Vec::new();

    for block in blocks {
        match block.get("type").and_then(Value::as_str).unwrap_or("") {
            "text" => {
                if let Some(text) = block.get("text").and_then(Value::as_str)
                    && !text.is_empty()
                {
                    text_parts.push(text.to_string());
                }
            }
            "image" => {
                if let Some(image) = image_part(block) {
                    images.push(image);
                }
            }
            "document" => {
                text_parts.push(document_text(block));
            }
            "tool_result" => {
                let id = block
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if id.is_empty() {
                    continue;
                }
                let mut text = flatten_content(block.get("content").unwrap_or(&Value::Null));
                if block
                    .get("is_error")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    text = format!("Error: {text}");
                }
                out.push(json!({"role": "tool", "tool_call_id": id, "content": text}));
            }
            // thinking/server_tool_use в пользовательском сообщении не переводятся.
            _ => {}
        }
    }

    if text_parts.is_empty() && images.is_empty() {
        return;
    }
    if images.is_empty() {
        out.push(json!({"role": "user", "content": text_parts.join("\n\n")}));
    } else {
        let mut parts = Vec::new();
        if !text_parts.is_empty() {
            parts.push(json!({"type": "text", "text": text_parts.join("\n\n")}));
        }
        parts.extend(images);
        out.push(json!({"role": "user", "content": parts}));
    }
}

fn convert_assistant(message: &Value, out: &mut Vec<Value>) {
    let Some(content) = message.get("content") else {
        return;
    };
    if let Value::String(text) = content {
        if !text.is_empty() {
            out.push(json!({"role": "assistant", "content": text}));
        }
        return;
    }
    let Some(blocks) = content.as_array() else {
        return;
    };

    let mut texts = Vec::new();
    let mut tool_calls = Vec::new();
    for block in blocks {
        match block.get("type").and_then(Value::as_str).unwrap_or("") {
            "text" => {
                if let Some(text) = block.get("text").and_then(Value::as_str)
                    && !text.is_empty()
                {
                    texts.push(text.to_string());
                }
            }
            "tool_use" => {
                let name = block.get("name").and_then(Value::as_str).unwrap_or("");
                if name.is_empty() {
                    continue;
                }
                let id = block
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("toolu_{}", Uuid::new_v4().simple()));
                let input = block.get("input").cloned().unwrap_or_else(|| json!({}));
                let arguments = serde_json::to_string(&input).unwrap_or_else(|_| "{}".into());
                tool_calls.push(json!({
                    "id": id,
                    "type": "function",
                    "function": {"name": name, "arguments": arguments}
                }));
            }
            // thinking/redacted_thinking бесполезны для OpenAI-провайдера.
            _ => {}
        }
    }

    if !tool_calls.is_empty() {
        let content = if texts.is_empty() {
            Value::Null
        } else {
            Value::String(texts.join("\n\n"))
        };
        out.push(json!({
            "role": "assistant",
            "content": content,
            "tool_calls": tool_calls
        }));
    } else if !texts.is_empty() {
        out.push(json!({"role": "assistant", "content": texts.join("\n\n")}));
    }
}

fn flatten_content(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| match part.get("type").and_then(Value::as_str) {
                Some("text") => part.get("text").and_then(Value::as_str).map(str::to_string),
                Some("image") => Some("[image]".to_string()),
                Some("document") => Some("[document]".to_string()),
                _ => part.get("text").and_then(Value::as_str).map(str::to_string),
            })
            .collect::<Vec<_>>()
            .join("\n\n"),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn document_text(block: &Value) -> String {
    let source = block.get("source");
    if let Some(text) = source
        .and_then(|source| source.get("data"))
        .and_then(Value::as_str)
        && source
            .and_then(|source| source.get("type"))
            .and_then(Value::as_str)
            == Some("text")
    {
        return text.to_string();
    }
    "[document]".to_string()
}

fn image_part(block: &Value) -> Option<Value> {
    let source = block.get("source")?;
    match source.get("type").and_then(Value::as_str) {
        Some("base64") => {
            let media = source
                .get("media_type")
                .and_then(Value::as_str)
                .unwrap_or("image/png");
            let data = source.get("data").and_then(Value::as_str)?;
            Some(json!({
                "type": "image_url",
                "image_url": {"url": format!("data:{media};base64,{data}")}
            }))
        }
        Some("url") => {
            let url = source.get("url").and_then(Value::as_str)?;
            Some(json!({"type": "image_url", "image_url": {"url": url}}))
        }
        _ => None,
    }
}

fn convert_tools(body: &Value) -> Vec<Value> {
    body.get("tools")
        .and_then(Value::as_array)
        .map(|tools| {
            tools
                .iter()
                .filter_map(|tool| {
                    let name = tool.get("name").and_then(Value::as_str)?;
                    let schema = portable_schema(tool.get("input_schema")?);
                    Some(json!({
                        "type": "function",
                        "function": {
                            "name": name,
                            "description": tool.get("description").and_then(Value::as_str).unwrap_or(""),
                            "parameters": schema
                        }
                    }))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Схема инструмента без `pattern`, которые не скомпилирует движок вида RE2. Провайдеры
/// на Go (OpenCode и др.) проверяют схемы им и отвергают весь запрос (400 «is not a
/// regex») из-за одного инструмента MCP, например с `^[^\uD800-\uDFFF]*$` (суррогаты)
/// или с просмотром вперёд из JS. Шаблон — лишь подсказка модели: входные данные всё
/// равно проверяет сам инструмент. Удаляются только строковые `pattern`: свойство с
/// именем `pattern` внутри `properties` — объект-схема, его не трогаем.
fn portable_schema(schema: &Value) -> Value {
    match schema {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(key, value)| {
                    !(key.as_str() == "pattern"
                        && value
                            .as_str()
                            .is_some_and(|re| regex::Regex::new(re).is_err()))
                })
                .map(|(key, value)| (key.clone(), portable_schema(value)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(portable_schema).collect()),
        other => other.clone(),
    }
}

fn convert_tool_choice(value: Option<&Value>, has_tools: bool) -> (Option<Value>, Option<bool>) {
    if !has_tools {
        return (None, None);
    }
    let Some(value) = value else {
        return (None, None);
    };
    let choice = match value.get("type").and_then(Value::as_str).unwrap_or("") {
        "auto" => Some(json!("auto")),
        "any" => Some(json!("required")),
        "none" => Some(json!("none")),
        "tool" => value
            .get("name")
            .and_then(Value::as_str)
            .map(|name| json!({"type": "function", "function": {"name": name}})),
        _ => None,
    };
    let parallel = value
        .get("disable_parallel_tool_use")
        .and_then(Value::as_bool)
        .map(|disabled| !disabled);
    (choice, parallel)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Provider, ProviderKind};
    use std::collections::BTreeMap;

    #[test]
    fn tool_schemas_drop_only_patterns_a_re2_engine_rejects() {
        let schema = json!({
            "type": "object",
            "properties": {
                // The rea MCP tool that made OpenCode answer 400 "is not a regex".
                "session": {"type": "string", "pattern": r"^[^\uD800-\uDFFF]*$"},
                "lookahead": {"type": "string", "pattern": "^(?=a)a$"},
                "plain": {"type": "string", "pattern": "^[a-z0-9-]+$"},
                // A property that is called "pattern" is a schema, not a regex.
                "pattern": {"type": "string", "description": "glob"},
                "list": {"type": "array", "items": {"anyOf": [
                    {"type": "string", "pattern": r"^[^\uD800-\uDFFF]*$"},
                    {"type": "integer"}
                ]}}
            }
        });
        let body = json!({"tools": [{"name": "t", "description": "d", "input_schema": schema}]});
        let out = convert_tools(&body);
        let props = &out[0]["function"]["parameters"]["properties"];
        assert!(props["session"].get("pattern").is_none());
        assert!(props["lookahead"].get("pattern").is_none());
        assert_eq!(props["plain"]["pattern"], "^[a-z0-9-]+$");
        assert_eq!(props["pattern"]["type"], "string");
        assert!(props["list"]["items"]["anyOf"][0].get("pattern").is_none());
        assert_eq!(props["session"]["type"], "string");
    }

    fn provider() -> Provider {
        Provider {
            id: "openai".into(),
            name: "OpenAI".into(),
            kind: ProviderKind::OpenAi,
            base_url: "https://api.openai.com/v1".into(),
            api_key: "sk-test".into(),
            headers: BTreeMap::new(),
            chat_completions_url: None,
            max_tokens_field: None,
            extra_body: Default::default(),
            include_usage: false,
            max_effort: None,
            default_effort: None,
            session_header: None,
        }
    }

    #[test]
    fn effort_becomes_reasoning_effort_only_when_enabled() {
        let body = json!({
            "model": "m", "max_tokens": 10,
            "thinking": {"type": "adaptive"},
            "output_config": {"effort": "max"},
            "messages": [{"role": "user", "content": "hi"}]
        });
        let off = build(&provider(), &body, false);
        assert!(off.get("reasoning_effort").is_none());
        assert!(off.get("output_config").is_none() && off.get("thinking").is_none());

        let on = Provider {
            max_effort: Some("xhigh"),
            ..provider()
        };
        assert_eq!(build(&on, &body, false)["reasoning_effort"], "xhigh");

        // extra_body провайдера по-прежнему главнее.
        let mut pinned = on.clone();
        pinned
            .extra_body
            .insert("reasoning_effort".into(), json!("low"));
        assert_eq!(build(&pinned, &body, false)["reasoning_effort"], "low");
    }

    #[test]
    fn converts_system_text_and_tools() {
        let body = json!({
            "model": "gpt-5.2",
            "max_tokens": 100,
            "system": [{"type": "text", "text": "You are Claude.", "cache_control": {"type": "ephemeral"}}],
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "hi"}]},
                {"role": "assistant", "content": [
                    {"type": "text", "text": "calling"},
                    {"type": "tool_use", "id": "toolu_1", "name": "read", "input": {"path": "a.txt"}},
                    {"type": "thinking", "thinking": "secret"}
                ]},
                {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_1", "content": "ok"}]}
            ],
            "tools": [{"name": "read", "description": "read a file", "input_schema": {"type": "object"}}],
            "tool_choice": {"type": "auto"}
        });
        let out = build(&provider(), &body, false);
        assert_eq!(out["messages"][0]["role"], "system");
        assert_eq!(out["messages"][1]["content"], "hi");
        assert_eq!(out["messages"][2]["tool_calls"][0]["id"], "toolu_1");
        assert_eq!(out["messages"][3]["role"], "tool");
        assert_eq!(out["tools"][0]["function"]["name"], "read");
        assert_eq!(out["max_completion_tokens"], 100);
    }

    #[test]
    fn maps_images_to_data_uri() {
        let body = json!({
            "messages": [{"role": "user", "content": [
                {"type": "text", "text": "look"},
                {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "AAAA"}}
            ]}]
        });
        let out = build(&provider(), &body, false);
        let content = out["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content[1]["image_url"]["url"], "data:image/png;base64,AAAA");
    }
}
