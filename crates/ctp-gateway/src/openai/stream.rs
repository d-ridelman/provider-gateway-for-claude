use crate::error;
use crate::openai::response;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use uuid::Uuid;

/// Рассуждение после начала текста короче этого (в символах) ждёт конца текста.
const LATE_REASONING_LIMIT: usize = 64;

#[derive(Clone, Copy, PartialEq, Eq)]
enum BlockKind {
    Text,
    Thinking,
}

struct CurrentBlock {
    kind: BlockKind,
    index: u32,
}

#[derive(Default)]
struct ToolCallBuf {
    id: String,
    name: String,
    arguments: String,
}

/// OpenAI Chat Completions SSE → Anthropic Messages SSE.
///
/// Текст и reasoning идут потоком; вызовы инструментов буферизуются до
/// `finish_reason`, потому что Anthropic не даёт дописывать в уже закрытый
/// content block, а OpenAI-провайдеры иногда чередуют дельты разных tool call.
pub struct StreamTranslator {
    model: String,
    message_id: String,
    input_tokens: u64,
    next_index: u32,
    current: Option<CurrentBlock>,
    tools: BTreeMap<u64, ToolCallBuf>,
    output_chars: u64,
    output_tokens: Option<u64>,
    /// Настоящие входные токены от провайдера (без кэша) и кэш — последнего отрезка:
    /// его запрос и есть нынешний контекст. Приходят в конце потока, уходят в
    /// `message_delta` вместо оценки из `message_start`.
    provider_input: Option<(u64, Option<u64>)>,
    /// Токены прошлых отрезков ответа (до вызовов советника).
    prior_output_tokens: u64,
    /// Текст текущего отрезка: из него шлюз собирает ответ ассистента для продолжения.
    segment_text: String,
    /// Рассуждение, пришедшее, когда текст ответа уже начался: пока оно короткое, текстовый
    /// блок не рвётся, а оно выходит отдельным блоком после текста (`close_current`).
    late_thinking: String,
    finish_reason: Option<String>,
    done: bool,
    secret: String,
    /// Сокращённое для провайдера имя инструмента → настоящее.
    names: std::collections::HashMap<String, String>,
}

/// Вызов советника, перехваченный у модели.
pub struct AdvisorCall {
    /// ID вызова у провайдера: с ним идёт продолжение (`tool_call_id`).
    pub call_id: String,
    /// ID блока `server_tool_use` для Claude Code.
    pub server_id: String,
    /// Текст отрезка до вызова — содержимое ответа ассистента в продолжении.
    pub text: String,
    /// Другие вызовы инструментов в том же ответе: тогда продолжения нет.
    pub others: bool,
}

impl StreamTranslator {
    pub fn new(model: impl Into<String>, input_tokens: u64) -> Self {
        Self {
            model: model.into(),
            message_id: format!("msg_{}", Uuid::new_v4().simple()),
            input_tokens,
            next_index: 0,
            current: None,
            tools: BTreeMap::new(),
            output_chars: 0,
            output_tokens: None,
            provider_input: None,
            prior_output_tokens: 0,
            segment_text: String::new(),
            late_thinking: String::new(),
            finish_reason: None,
            done: false,
            secret: String::new(),
            names: Default::default(),
        }
    }

    /// Настоящие имена инструментов, сокращённых для провайдера (`adapt::name_map`).
    pub fn with_names(mut self, names: std::collections::HashMap<String, String>) -> Self {
        self.names = names;
        self
    }

    /// Ключ провайдера: вырезается из ошибок, пришедших внутри потока.
    pub fn with_secret(mut self, secret: impl Into<String>) -> Self {
        self.secret = secret.into();
        self
    }

    pub fn start(&self) -> Value {
        json!({
            "type": "message_start",
            "message": {
                "id": self.message_id,
                "type": "message",
                "role": "assistant",
                "model": self.model,
                "content": [],
                "stop_reason": null,
                "stop_sequence": null,
                "usage": {"input_tokens": self.input_tokens, "output_tokens": 0}
            }
        })
    }

    pub fn on_chunk(&mut self, chunk: &Value) -> Vec<Value> {
        if self.done {
            return Vec::new();
        }
        if chunk.get("error").is_some() {
            self.done = true;
            // Статуса у ошибки в потоке нет: берём числовой `code`, а переполнение окна
            // опознаётся по тексту и приводится к 400.
            let text = chunk.to_string();
            let status = chunk
                .pointer("/error/code")
                .and_then(Value::as_u64)
                .and_then(|code| u16::try_from(code).ok())
                .unwrap_or(if crate::openai::adapt::is_overflow(&text) {
                    400
                } else {
                    502
                });
            let (_, kind, message) = error::classify(status, &text, &self.secret);
            return vec![Self::error_event_of(kind, &message)];
        }

        if let Some(output) = chunk
            .get("usage")
            .and_then(|usage| {
                usage
                    .get("completion_tokens")
                    .or_else(|| usage.get("output_tokens"))
            })
            .and_then(Value::as_u64)
        {
            self.output_tokens = Some(output);
        }
        if let Some(input) = chunk.get("usage").and_then(response::input_usage) {
            self.provider_input = Some(input);
        }

        let Some(choice) = chunk
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
        else {
            return Vec::new();
        };
        let delta = choice.get("delta").unwrap_or(&Value::Null);
        let mut events = Vec::new();

        let reasoning = reasoning_delta(delta);
        if !reasoning.is_empty() {
            self.output_chars += reasoning.chars().count() as u64;
            // Some providers send the tail of the reasoning (even a lone ".") after the first
            // token of the answer: opening a thinking block there cut "Пробую" into "П" and
            // "робую". A short straggler waits; a long one is real interleaved thinking.
            let in_text = self
                .current
                .as_ref()
                .is_some_and(|block| block.kind == BlockKind::Text);
            let held = self.late_thinking.chars().count() + reasoning.chars().count();
            if in_text && held <= LATE_REASONING_LIMIT {
                self.late_thinking.push_str(&reasoning);
            } else {
                // Real thinking after the text: what waited goes first, in the provider's order.
                let reasoning = std::mem::take(&mut self.late_thinking) + &reasoning;
                events.extend(self.ensure(BlockKind::Thinking));
                let index = self.current.as_ref().expect("block was opened").index;
                events.push(json!({
                    "type": "content_block_delta",
                    "index": index,
                    "delta": {"type": "thinking_delta", "thinking": reasoning}
                }));
            }
        }

        let text = content_delta(delta);
        if !text.is_empty() {
            events.extend(self.ensure(BlockKind::Text));
            let index = self.current.as_ref().expect("block was opened").index;
            events.push(json!({
                "type": "content_block_delta",
                "index": index,
                "delta": {"type": "text_delta", "text": text}
            }));
            self.output_chars += text.chars().count() as u64;
            self.segment_text.push_str(&text);
        }

        if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
            for (position, call) in calls.iter().enumerate() {
                let index = call
                    .get("index")
                    .and_then(Value::as_u64)
                    .unwrap_or(position as u64);
                let entry = self.tools.entry(index).or_default();
                if let Some(id) = call
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                {
                    entry.id = id.to_string();
                }
                if let Some(name) = call
                    .get("function")
                    .and_then(|function| function.get("name"))
                    .and_then(Value::as_str)
                    .filter(|name| !name.is_empty())
                {
                    entry.name = name.to_string();
                }
                if let Some(arguments) = call
                    .get("function")
                    .and_then(|function| function.get("arguments"))
                {
                    entry.arguments.push_str(&arguments_text(arguments));
                }
            }
        }

        if let Some(reason) = choice
            .get("finish_reason")
            .and_then(Value::as_str)
            .filter(|reason| !reason.is_empty())
        {
            self.finish_reason = Some(reason.to_string());
        }

        events
    }

    pub fn finish(&mut self) -> Vec<Value> {
        if self.done {
            return Vec::new();
        }
        self.done = true;

        let mut events = self.close_current();
        let mut tool_count = 0usize;
        for (_, mut call) in std::mem::take(&mut self.tools) {
            if call.name.is_empty() {
                tracing::warn!("tool call without a name skipped");
                continue;
            }
            if call.id.is_empty() {
                call.id = format!("toolu_{}", Uuid::new_v4().simple());
            }
            let arguments = valid_arguments(&call.arguments);
            let index = self.next_index;
            self.next_index += 1;
            events.push(json!({
                "type": "content_block_start",
                "index": index,
                "content_block": {"type": "tool_use", "id": call.id,
                                  "name": self.names.get(&call.name).unwrap_or(&call.name), "input": {}}
            }));
            events.push(json!({
                "type": "content_block_delta",
                "index": index,
                "delta": {"type": "input_json_delta", "partial_json": arguments}
            }));
            events.push(json!({"type": "content_block_stop", "index": index}));
            self.output_chars += arguments.chars().count() as u64;
            tool_count += 1;
        }

        let stop_reason = response::map_stop_reason(self.finish_reason.as_deref(), tool_count > 0);
        let output_tokens = self.prior_output_tokens + self.segment_tokens();
        let mut usage = json!({"output_tokens": output_tokens});
        if let Some((input, cached)) = self.provider_input {
            usage["input_tokens"] = json!(input);
            if let Some(cached) = cached {
                usage["cache_read_input_tokens"] = json!(cached);
            }
        }
        events.push(json!({
            "type": "message_delta",
            "delta": {"stop_reason": stop_reason, "stop_sequence": null},
            "usage": usage
        }));
        events.push(json!({"type": "message_stop"}));
        events
    }

    pub fn is_done(&self) -> bool {
        self.done
    }

    fn segment_tokens(&self) -> u64 {
        self.output_tokens
            .unwrap_or_else(|| (self.output_chars / 4).max(1))
    }

    /// Модель вызвала советника: вызов убирается из буфера, Claude Code получает блок
    /// `server_tool_use` (как от сервера Anthropic). Остальные вызовы остаются в буфере.
    pub fn take_advisor_call(&mut self) -> Option<(AdvisorCall, Vec<Value>)> {
        if self.done {
            return None;
        }
        let keys: Vec<u64> = self
            .tools
            .iter()
            .filter(|(_, call)| call.name == crate::openai::request::ADVISOR)
            .map(|(key, _)| *key)
            .collect();
        let call = self.tools.remove(keys.first()?)?;
        // Повторный вызов в том же ответе — тот же совет: лишние убираются, иначе они
        // ушли бы в Claude Code вызовом незнакомого ему инструмента.
        for key in &keys[1..] {
            self.tools.remove(key);
        }
        // ID вернётся в истории как `tool_call_id`: у OpenAI предел — 40 символов.
        let server_id = format!("srvtoolu_{}", &Uuid::new_v4().simple().to_string()[..24]);
        let call_id = if call.id.is_empty() {
            format!("call_{}", Uuid::new_v4().simple())
        } else {
            call.id
        };
        let mut events = self.close_current();
        let index = self.next_index;
        self.next_index += 1;
        events.push(json!({
            "type": "content_block_start",
            "index": index,
            "content_block": {"type": "server_tool_use", "id": server_id, "name": crate::openai::request::ADVISOR, "input": {}}
        }));
        events.push(json!({
            "type": "content_block_delta",
            "index": index,
            "delta": {"type": "input_json_delta", "partial_json": ""}
        }));
        events.push(json!({"type": "content_block_stop", "index": index}));
        let advisor = AdvisorCall {
            call_id,
            server_id,
            text: std::mem::take(&mut self.segment_text),
            others: !self.tools.is_empty(),
        };
        Some((advisor, events))
    }

    /// Ответ советника для Claude Code: блок `advisor_tool_result`.
    pub fn advisor_result(&mut self, server_id: &str, content: Value) -> Vec<Value> {
        let index = self.next_index;
        self.next_index += 1;
        vec![
            json!({
                "type": "content_block_start",
                "index": index,
                "content_block": {"type": "advisor_tool_result", "tool_use_id": server_id, "content": content}
            }),
            json!({"type": "content_block_stop", "index": index}),
        ]
    }

    /// Новый отрезок того же ответа (продолжение после советника): блоки и номера идут
    /// дальше, токены складываются.
    pub fn next_segment(&mut self) {
        self.prior_output_tokens += self.segment_tokens();
        self.output_tokens = None;
        // Размер запроса — только того отрезка, что сообщит его сам (иначе — оценка).
        self.provider_input = None;
        self.output_chars = 0;
        self.segment_text.clear();
        self.late_thinking.clear();
        self.finish_reason = None;
    }

    pub fn error_event(message: &str) -> Value {
        Self::error_event_of("api_error", message)
    }

    /// Ошибка внутри потока с типом по статусу провайдера (лимит, переполнение окна…).
    pub fn error_event_of(kind: &str, message: &str) -> Value {
        json!({"type": "error", "error": {"type": kind, "message": message}})
    }

    fn ensure(&mut self, kind: BlockKind) -> Vec<Value> {
        if self
            .current
            .as_ref()
            .is_some_and(|block| block.kind == kind)
        {
            return Vec::new();
        }
        let mut events = self.close_current();
        let index = self.next_index;
        self.next_index += 1;
        let content_block = match kind {
            BlockKind::Text => json!({"type": "text", "text": ""}),
            BlockKind::Thinking => json!({"type": "thinking", "thinking": ""}),
        };
        events.push(json!({
            "type": "content_block_start",
            "index": index,
            "content_block": content_block
        }));
        self.current = Some(CurrentBlock { kind, index });
        events
    }

    fn close_current(&mut self) -> Vec<Value> {
        let Some(block) = self.current.take() else {
            return Vec::new();
        };
        let mut events = vec![json!({"type": "content_block_stop", "index": block.index})];
        if block.kind == BlockKind::Text && !self.late_thinking.is_empty() {
            // Marks without a letter or digit (".") carry nothing: dropped. Words are kept, as a
            // block of their own right after the text.
            let held = std::mem::take(&mut self.late_thinking);
            let keep = held.chars().any(char::is_alphanumeric);
            tracing::info!(
                chars = held.chars().count(),
                kept = keep,
                "reasoning arrived after the answer text began"
            );
            if keep {
                let index = self.next_index;
                self.next_index += 1;
                events.push(json!({
                    "type": "content_block_start",
                    "index": index,
                    "content_block": {"type": "thinking", "thinking": ""}
                }));
                events.push(json!({
                    "type": "content_block_delta",
                    "index": index,
                    "delta": {"type": "thinking_delta", "thinking": held}
                }));
                events.push(json!({"type": "content_block_stop", "index": index}));
            }
        }
        events
    }
}

fn reasoning_delta(delta: &Value) -> String {
    for key in ["reasoning_content", "reasoning"] {
        if let Some(text) = delta.get(key).and_then(Value::as_str)
            && !text.is_empty()
        {
            return text.to_string();
        }
    }
    if let Some(details) = delta.get("reasoning_details").and_then(Value::as_array) {
        return details
            .iter()
            .filter_map(|detail| detail.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("");
    }
    String::new()
}

fn content_delta(delta: &Value) -> String {
    match delta.get("content") {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

fn arguments_text(arguments: &Value) -> String {
    match arguments {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

fn valid_arguments(arguments: &str) -> String {
    if arguments.is_empty() {
        return "{}".to_string();
    }
    match serde_json::from_str::<Value>(arguments) {
        Ok(Value::Object(_)) => arguments.to_string(),
        _ => {
            tracing::warn!("tool arguments are not a JSON object, sending {{}}");
            "{}".to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_inside_stream_does_not_leak_odd_key() {
        let mut translator = StreamTranslator::new("claude-x", 1).with_secret("sk-abcdef+ghi/jkl");
        let events =
            translator.on_chunk(&json!({"error": {"message": "bad key sk-abcdef+ghi/jkl"}}));
        let text = events[0].to_string();
        assert!(!text.contains("ghi/") && !text.contains("abcdef"), "{text}");
    }

    #[test]
    fn translates_text_thinking_and_buffered_tool_call() {
        let mut translator = StreamTranslator::new("claude-x", 12);
        let start = translator.start();
        assert_eq!(start["type"], "message_start");

        let chunk = json!({
            "choices": [{
                "index": 0,
                "delta": {
                    "reasoning_content": "think",
                    "content": "hi",
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_1",
                        "function": {"name": "read", "arguments": "{\"path\":"}
                    }]
                },
                "finish_reason": null
            }]
        });
        let events = translator.on_chunk(&chunk);
        let types: Vec<&str> = events
            .iter()
            .map(|event| event["type"].as_str().unwrap())
            .collect();
        assert_eq!(
            types,
            vec![
                "content_block_start",
                "content_block_delta",
                "content_block_stop",
                "content_block_start",
                "content_block_delta"
            ]
        );

        let finish = json!({
            "choices": [{
                "delta": {"tool_calls": [{"index": 0, "function": {"arguments": "\"a.txt\"}"}}]},
                "finish_reason": "tool_calls"
            }],
            "usage": {"prompt_tokens": 12, "completion_tokens": 7}
        });
        let events = translator.on_chunk(&finish);
        assert!(events.is_empty(), "tool delta не должен литься в SSE сразу");

        let events = translator.finish();
        let types: Vec<&str> = events
            .iter()
            .map(|event| event["type"].as_str().unwrap())
            .collect();
        assert_eq!(
            types,
            vec![
                "content_block_stop",
                "content_block_start",
                "content_block_delta",
                "content_block_stop",
                "message_delta",
                "message_stop"
            ]
        );
        assert_eq!(events[4]["usage"]["output_tokens"], 7);
        // Настоящий размер запроса от провайдера — Claude Code считает контекст по нему.
        assert_eq!(events[4]["usage"]["input_tokens"], 12);
        assert!(events[4]["usage"].get("cache_read_input_tokens").is_none());
        assert_eq!(events[4]["delta"]["stop_reason"], "tool_use");
    }

    #[test]
    fn provider_input_is_reported_without_the_cache_twice() {
        let mut translator = StreamTranslator::new("claude-x", 999_999);
        translator.on_chunk(&json!({
            "choices": [{"index": 0, "delta": {"content": "hi"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 50_000, "completion_tokens": 3,
                      "prompt_tokens_details": {"cached_tokens": 40_000}}
        }));
        let events = translator.finish();
        let delta = events
            .iter()
            .find(|e| e["type"] == "message_delta")
            .unwrap();
        // OpenAI: prompt_tokens включает кэш; Anthropic: input_tokens — без него.
        assert_eq!(delta["usage"]["input_tokens"], 10_000);
        assert_eq!(delta["usage"]["cache_read_input_tokens"], 40_000);

        // Провайдер без usage или с нулями: оценка остаётся в message_start, в delta
        // входных нет (ноль сказал бы Claude Code, что контекст пуст).
        for usage in [
            Value::Null,
            json!({"prompt_tokens": 0, "completion_tokens": 3}),
            json!({"input_tokens": 0, "output_tokens": 3}),
        ] {
            let mut quiet = StreamTranslator::new("claude-x", 123);
            let mut chunk = json!({"choices": [{"index": 0, "delta": {"content": "hi"}, "finish_reason": "stop"}]});
            if !usage.is_null() {
                chunk["usage"] = usage;
            }
            quiet.on_chunk(&chunk);
            let events = quiet.finish();
            let delta = events
                .iter()
                .find(|e| e["type"] == "message_delta")
                .unwrap();
            assert!(delta["usage"].get("input_tokens").is_none(), "{delta}");
        }

        // Anthropic-поля: input_tokens без кэша, кэш отдельно и не вычитается.
        assert_eq!(
            response::input_usage(&json!({"input_tokens": 100, "cache_read_input_tokens": 900})),
            Some((100, Some(900)))
        );
        // Ноль в последнем чанке не затирает цифру из предыдущего.
        let mut late = StreamTranslator::new("claude-x", 1);
        late.on_chunk(&json!({"choices": [], "usage": {"prompt_tokens": 7000}}));
        late.on_chunk(
            &json!({"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
                              "usage": {"prompt_tokens": 0}}),
        );
        let events = late.finish();
        let delta = events
            .iter()
            .find(|e| e["type"] == "message_delta")
            .unwrap();
        assert_eq!(delta["usage"]["input_tokens"], 7000);
    }

    fn feed(translator: &mut StreamTranslator, deltas: &[Value]) -> Vec<Value> {
        let mut events = Vec::new();
        for delta in deltas {
            events.extend(translator.on_chunk(&json!({
                "choices": [{"index": 0, "delta": delta, "finish_reason": null}]
            })));
        }
        events
    }

    /// Kinds of the blocks, in the order they are opened.
    fn block_kinds(events: &[Value]) -> Vec<String> {
        events
            .iter()
            .filter(|event| event["type"] == "content_block_start")
            .map(|event| event["content_block"]["type"].as_str().unwrap().to_string())
            .collect()
    }

    /// Everything streamed as `text_delta` / `thinking_delta` (field `text` / `thinking`).
    fn streamed(events: &[Value], field: &str) -> String {
        events
            .iter()
            .filter_map(|event| event["delta"][field].as_str())
            .collect()
    }

    fn tool_call(name: &str) -> Value {
        json!({"tool_calls": [{"index": 0, "id": "c1", "function": {"name": name, "arguments": "{}"}}]})
    }

    #[test]
    fn a_stray_reasoning_mark_inside_the_answer_does_not_split_it() {
        // As a provider sent it (Code tab transcript): reasoning, text "П", reasoning ".",
        // text "робую…", a tool call. Desktop showed "П", a thinking ".", then "робую…".
        let mut translator = StreamTranslator::new("claude-x", 1);
        let deltas = [
            json!({"reasoning_content": "The user asks to find the stream"}),
            json!({"content": "П"}),
            json!({"reasoning_content": "."}),
            json!({"content": "робую по найденному API.\n\n"}),
            tool_call("Bash"),
        ];
        let mut events = feed(&mut translator, &deltas);
        events.extend(translator.finish());
        assert_eq!(block_kinds(&events), ["thinking", "text", "tool_use"]);
        assert_eq!(streamed(&events, "text"), "Пробую по найденному API.\n\n");
        assert_eq!(
            streamed(&events, "thinking"),
            "The user asks to find the stream"
        );
        // The dropped mark still counts as output, as before.
        let chars: usize = deltas
            .iter()
            .map(|d| {
                ["reasoning_content", "content"]
                    .iter()
                    .filter_map(|key| d[key].as_str())
                    .map(|s| s.chars().count())
                    .sum::<usize>()
                    + usize::from(d.get("tool_calls").is_some()) * 2
            })
            .sum();
        let delta = events
            .iter()
            .find(|e| e["type"] == "message_delta")
            .unwrap();
        assert_eq!(delta["usage"]["output_tokens"], (chars as u64 / 4).max(1));
    }

    #[test]
    fn late_reasoning_with_words_follows_the_answer_text() {
        let mut translator = StreamTranslator::new("claude-x", 1);
        let mut events = feed(
            &mut translator,
            &[
                json!({"reasoning_content": "plan"}),
                json!({"content": "Hi"}),
                json!({"reasoning_content": " then check"}),
                json!({"content": " there"}),
            ],
        );
        events.extend(translator.finish());
        assert_eq!(block_kinds(&events), ["thinking", "text", "thinking"]);
        assert_eq!(streamed(&events, "text"), "Hi there");
        assert_eq!(streamed(&events, "thinking"), "plan then check");
        // The text block is closed before the late thinking opens.
        let types: Vec<&str> = events.iter().map(|e| e["type"].as_str().unwrap()).collect();
        assert_eq!(
            types,
            [
                "content_block_start",
                "content_block_delta",
                "content_block_stop", // thinking
                "content_block_start",
                "content_block_delta",
                "content_block_delta",
                "content_block_stop", // text
                "content_block_start",
                "content_block_delta",
                "content_block_stop", // late thinking
                "message_delta",
                "message_stop"
            ]
        );
    }

    #[test]
    fn long_reasoning_between_texts_keeps_the_providers_order() {
        // A provider that really interleaves thinking and text is not rearranged.
        let mut translator = StreamTranslator::new("claude-x", 1);
        let long = "x ".repeat(100);
        let mut events = feed(
            &mut translator,
            &[
                json!({"reasoning_content": "plan"}),
                json!({"content": "Hi"}),
                json!({"reasoning_content": long}),
                json!({"content": "Done"}),
            ],
        );
        events.extend(translator.finish());
        assert_eq!(
            block_kinds(&events),
            ["thinking", "text", "thinking", "text"]
        );
        assert_eq!(streamed(&events, "thinking"), format!("plan{long}"));
    }

    #[test]
    fn late_reasoning_is_flushed_before_an_advisor_call() {
        let mut translator = StreamTranslator::new("claude-x", 1);
        feed(
            &mut translator,
            &[
                json!({"reasoning_content": "plan"}),
                json!({"content": "Hi"}),
                json!({"reasoning_content": " ask them"}),
                tool_call(crate::openai::request::ADVISOR),
            ],
        );
        let (_, events) = translator.take_advisor_call().expect("advisor call");
        assert_eq!(block_kinds(&events), ["thinking", "server_tool_use"]);
        assert_eq!(streamed(&events, "thinking"), " ask them");
    }
}
