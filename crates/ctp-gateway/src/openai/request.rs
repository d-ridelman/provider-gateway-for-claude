use crate::state::Provider;
use serde_json::{Map, Value, json};
use uuid::Uuid;

const IMAGE_OMITTED: &str = "[image omitted: this model does not accept images]";
/// PDF (base64 или по ссылке) модель OpenAI-провайдера не прочтёт: говорим об этом прямо,
/// иначе на голое `[document]` она пересказывает файл, которого не видела.
const PDF_OMITTED: &str = "[PDF omitted: this model cannot read PDF documents]";

/// Есть ли в сообщениях картинки (`image_url`).
pub fn has_images(payload: &Value) -> bool {
    payload
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|message| message.get("content").and_then(Value::as_array))
        .flatten()
        .any(|part| part.get("type").and_then(Value::as_str) == Some("image_url"))
}

/// Картинки заменяются пометкой в тексте (одной на подряд идущие). Часть `glm-5.3`,
/// например, отвечает на любую картинку 400 «the request is invalid»: сессия со
/// скриншотами в истории тогда не работает вовсе. Сколько картинок убрано.
pub fn strip_images(payload: &mut Value) -> usize {
    let mut removed = 0;
    let Some(messages) = payload.get_mut("messages").and_then(Value::as_array_mut) else {
        return 0;
    };
    for message in messages {
        let Some(parts) = message.get("content").and_then(Value::as_array) else {
            continue;
        };
        if !parts
            .iter()
            .any(|part| part.get("type").and_then(Value::as_str) == Some("image_url"))
        {
            continue;
        }
        let mut texts: Vec<String> = Vec::new();
        for part in parts {
            if part.get("type").and_then(Value::as_str) == Some("image_url") {
                removed += 1;
                if texts.last().map(String::as_str) != Some(IMAGE_OMITTED) {
                    texts.push(IMAGE_OMITTED.to_string());
                }
            } else if let Some(text) = part.get("text").and_then(Value::as_str) {
                texts.push(text.to_string());
            }
        }
        message["content"] = Value::String(texts.join("\n\n"));
    }
    removed
}

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

    let mut payload = Value::Object(request);
    // Рассуждение прошлых ответов нужно только запросу с инструментами (DeepSeek без них его
    // игнорирует, другие провайдеры могут отказать на незнакомом поле).
    if !has_tools {
        strip_reasoning(&mut payload);
    }
    payload
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
                let result = block.get("content").unwrap_or(&Value::Null);
                let mut text = flatten_content(result);
                // В сообщении `tool` у OpenAI только текст: картинки инструмента (скриншоты)
                // идут следом сообщением пользователя, иначе модель увидела бы слово «[image]».
                if let Value::Array(parts) = result {
                    let before = images.len();
                    images.extend(
                        parts
                            .iter()
                            .filter(|part| {
                                part.get("type").and_then(Value::as_str) == Some("image")
                            })
                            .filter_map(image_part),
                    );
                    if images.len() > before && text_parts.is_empty() {
                        text_parts
                            .push("The images returned by the tool results above.".to_string());
                    }
                }
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
    // Рассуждение ответа (блоки `thinking`): DeepSeek в режиме рассуждения при запросе с
    // инструментами требует `reasoning_content` прошлых ответов назад, иначе 400. Claude Code
    // присылает блоки в истории.
    let mut reasoning = Vec::new();
    for block in blocks {
        match block.get("type").and_then(Value::as_str).unwrap_or("") {
            "thinking" => {
                if let Some(text) = block.get("thinking").and_then(Value::as_str)
                    && !text.is_empty()
                {
                    reasoning.push(text.to_string());
                }
            }
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
            // redacted_thinking бесполезен для OpenAI-провайдера.
            _ => {}
        }
    }
    flush_assistant(&mut texts, &mut tool_calls, &mut reasoning, out);
}

/// Ответ ассистента из накопленного. Рассуждение уходит вместе с ним полем `reasoning_content`
/// (прикладывается к первому сообщению, которое выходит из блоков); ответ из одних
/// рассуждений ничего не выпускает.
fn flush_assistant(
    texts: &mut Vec<String>,
    tool_calls: &mut Vec<Value>,
    reasoning: &mut Vec<String>,
    out: &mut Vec<Value>,
) {
    let mut message = if !tool_calls.is_empty() {
        let content = if texts.is_empty() {
            Value::Null
        } else {
            Value::String(texts.join("\n\n"))
        };
        json!({
            "role": "assistant",
            "content": content,
            "tool_calls": std::mem::take(tool_calls)
        })
    } else if !texts.is_empty() {
        json!({"role": "assistant", "content": texts.join("\n\n")})
    } else {
        return;
    };
    if !reasoning.is_empty() {
        message["reasoning_content"] = json!(reasoning.join("\n\n"));
        reasoning.clear();
    }
    out.push(message);
    texts.clear();
}

/// Убирает `reasoning_content` из сообщений: без `tools` DeepSeek его игнорирует, а другие
/// провайдеры могут отказать на незнакомом поле. Сколько сообщений изменилось.
pub fn strip_reasoning(payload: &mut Value) -> usize {
    take_reasoning(payload).len()
}

/// То же, но вынутое возвращается — (позиция сообщения, значение): `restore_reasoning` вернёт
/// его, если запрос без поля всё равно не прошёл.
pub fn take_reasoning(payload: &mut Value) -> Vec<(usize, Value)> {
    let Some(messages) = payload.get_mut("messages").and_then(Value::as_array_mut) else {
        return Vec::new();
    };
    messages
        .iter_mut()
        .enumerate()
        .filter_map(|(at, message)| {
            message
                .as_object_mut()?
                .remove("reasoning_content")
                .map(|value| (at, value))
        })
        .collect()
}

pub fn restore_reasoning(payload: &mut Value, saved: Vec<(usize, Value)>) {
    let Some(messages) = payload.get_mut("messages").and_then(Value::as_array_mut) else {
        return;
    };
    for (at, value) in saved {
        if let Some(message) = messages.get_mut(at).and_then(Value::as_object_mut) {
            message.insert("reasoning_content".into(), value);
        }
    }
}

fn flatten_content(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| match part.get("type").and_then(Value::as_str) {
                Some("text") => part.get("text").and_then(Value::as_str).map(str::to_string),
                // Картинку, которую `convert_user` перенесёт в сообщение пользователя, так и называем.
                Some("image") => Some(
                    if image_part(part).is_some() {
                        "[image attached below]"
                    } else {
                        "[image]"
                    }
                    .to_string(),
                ),
                Some("document") => Some(PDF_OMITTED.to_string()),
                // Найденный ToolSearch инструмент (как у Ollama): сам он уже в списке `tools`,
                // модели нужно имя, иначе результат поиска для неё пуст.
                Some("tool_reference") => part
                    .get("tool_name")
                    .and_then(Value::as_str)
                    .map(|name| format!("Tool loaded: {name}")),
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
    PDF_OMITTED.to_string()
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
    // Повтор имени провайдеры отвергают (400 «the request is invalid»): остаётся первый.
    let mut seen = std::collections::HashSet::new();
    body.get("tools")
        .and_then(Value::as_array)
        .map(|tools| {
            tools
                .iter()
                .filter_map(|tool| {
                    let name = tool.get("name").and_then(Value::as_str)?;
                    // Серверный инструмент без схемы (советник) имени не занимает.
                    let schema = portable_schema(tool.get("input_schema")?);
                    if !seen.insert(name.to_string()) {
                        return None;
                    }
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

/// Схема инструмента без регулярок, которые не примет движок вида RE2 (Go `regexp`).
/// Провайдеры проверяют ими схемы и отвергают весь запрос (400 «is not a regex») из-за
/// одного инструмента MCP: например, `^[^\uD800-\uDFFF]*$` (суррогаты) или шаблоны из
/// JS с `\uXXXX` и просмотром вперёд. Шаблон — лишь подсказка модели: входные данные
/// всё равно проверяет сам инструмент. Чистятся строковые `pattern` и ключи
/// `patternProperties`. Ключи читаются как ключевые слова только в самой схеме: в
/// картах «имя → схема» (`properties`, `$defs`…) имена — это просто имена свойств, а в
/// данные (`default`, `enum`, `examples`, `const`) не заходим вовсе.
fn portable_schema(schema: &Value) -> Value {
    let Value::Object(map) = schema else {
        // `items: [..]`, `true`/`false` и прочее без ключевых слов.
        return match schema {
            Value::Array(items) => Value::Array(items.iter().map(portable_schema).collect()),
            other => other.clone(),
        };
    };
    let named = |value: &Value, keep: &dyn Fn(&str) -> bool| match value {
        Value::Object(entries) => Value::Object(
            entries
                .iter()
                .filter(|(name, _)| keep(name))
                .map(|(name, sub)| (name.clone(), portable_schema(sub)))
                .collect(),
        ),
        other => other.clone(),
    };
    Value::Object(
        map.iter()
            .filter(|(key, value)| {
                !(key.as_str() == "pattern" && value.as_str().is_some_and(|re| !portable_regex(re)))
            })
            .map(|(key, value)| {
                let value = match key.as_str() {
                    "default" | "enum" | "examples" | "example" | "const" => value.clone(),
                    "properties" | "$defs" | "definitions" | "dependentSchemas" => {
                        named(value, &|_| true)
                    }
                    "patternProperties" => named(value, &portable_regex),
                    _ => portable_schema(value),
                };
                (key.clone(), value)
            })
            .collect(),
    )
}

/// Регулярка, которую примет Go `regexp` (синтаксис RE2). Разбор в дерево, а не
/// компиляция: микросекунды даже на сотнях инструментов. Дерево Rust шире Go, поэтому
/// поверх разбора — его ограничения (`go_compatible`).
// shortcut: редкий синтаксис Go, которого нет в Rust (`\Q…\E`, `\012`, `a{,5}`), отбрасывается — так только теряется подсказка; добавить, если провайдеры начнут на него опираться.
fn portable_regex(re: &str) -> bool {
    regex_syntax::ast::parse::Parser::new()
        .parse(re)
        .is_ok_and(|ast| go_compatible(&ast, 1))
}

/// Ограничения Go `regexp/syntax`, которых нет в Rust: повтор не больше 1000 (и
/// произведение вложенных тоже), без повтора повтора (`a++`, `a**`), флаги только
/// `i m s U`, без `\U…`, без операций над классами (`&&`, `--`, `~~`), без `\b{…}` и
/// свойств Unicode, которых нет в таблицах Go (`\p{Emoji}`, `\p{sc=Latin}`).
fn go_compatible(ast: &regex_syntax::ast::Ast, outer: u64) -> bool {
    use regex_syntax::ast::{AssertionKind, Ast, GroupKind, RepetitionKind, RepetitionRange};
    match ast {
        Ast::Empty(_) | Ast::Dot(_) | Ast::ClassPerl(_) => true,
        Ast::Flags(set) => go_flags(&set.flags),
        Ast::Literal(literal) => go_literal(literal),
        Ast::Assertion(assertion) => matches!(
            assertion.kind,
            AssertionKind::StartLine
                | AssertionKind::EndLine
                | AssertionKind::StartText
                | AssertionKind::EndText
                | AssertionKind::WordBoundary
                | AssertionKind::NotWordBoundary
        ),
        Ast::ClassUnicode(class) => go_unicode_class(&class.kind),
        Ast::ClassBracketed(class) => go_class_set(&class.kind),
        Ast::Repetition(rep) => {
            if matches!(*rep.ast, Ast::Repetition(_)) {
                return false;
            }
            let count = match &rep.op.kind {
                RepetitionKind::Range(
                    RepetitionRange::Exactly(n)
                    | RepetitionRange::AtLeast(n)
                    | RepetitionRange::Bounded(_, n),
                ) => u64::from(*n),
                _ => 1,
            };
            let total = outer.saturating_mul(count.max(1));
            count <= 1000 && total <= 1000 && go_compatible(&rep.ast, total)
        }
        Ast::Group(group) => {
            let flags_ok = match &group.kind {
                GroupKind::NonCapturing(flags) => go_flags(flags),
                // Go: имя группы — только [A-Za-z0-9_].
                GroupKind::CaptureName { name, .. } => name
                    .name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_'),
                GroupKind::CaptureIndex(_) => true,
            };
            flags_ok && go_compatible(&group.ast, outer)
        }
        Ast::Alternation(alt) => alt.asts.iter().all(|a| go_compatible(a, outer)),
        Ast::Concat(cat) => cat.asts.iter().all(|a| go_compatible(a, outer)),
    }
}

/// `\u0041`, `\U0001F600` и их `{…}` — только в Rust; в Go — `\x41`, `\x{1F600}`.
fn go_literal(literal: &regex_syntax::ast::Literal) -> bool {
    use regex_syntax::ast::{HexLiteralKind, LiteralKind};
    !matches!(
        literal.kind,
        LiteralKind::HexFixed(HexLiteralKind::UnicodeShort | HexLiteralKind::UnicodeLong)
            | LiteralKind::HexBrace(HexLiteralKind::UnicodeShort | HexLiteralKind::UnicodeLong)
    )
}

fn go_flags(flags: &regex_syntax::ast::Flags) -> bool {
    use regex_syntax::ast::{Flag, FlagsItemKind};
    flags.items.iter().all(|item| match item.kind {
        FlagsItemKind::Negation => true,
        FlagsItemKind::Flag(flag) => matches!(
            flag,
            Flag::CaseInsensitive | Flag::MultiLine | Flag::DotMatchesNewLine | Flag::SwapGreed
        ),
    })
}

/// `\pL`, `\p{Lu}` (категории), `\p{Any}` и письменности есть в Go; свойства вида
/// `\p{Alphabetic}`, `\p{Emoji}` и `name=value` — только в Rust.
fn go_unicode_class(kind: &regex_syntax::ast::ClassUnicodeKind) -> bool {
    use regex_syntax::ast::ClassUnicodeKind;
    // Письменности Unicode 6.0 — таблицы Go с 1.0, есть у провайдера на любой версии Go.
    // Имена с учётом регистра: без него Go понимает их только с 1.25.
    const GO_NAMES: &[&str] = &[
        "Any",
        "Arabic",
        "Armenian",
        "Avestan",
        "Balinese",
        "Bamum",
        "Batak",
        "Bengali",
        "Bopomofo",
        "Brahmi",
        "Braille",
        "Buginese",
        "Buhid",
        "Canadian_Aboriginal",
        "Carian",
        "Cham",
        "Cherokee",
        "Common",
        "Coptic",
        "Cuneiform",
        "Cypriot",
        "Cyrillic",
        "Deseret",
        "Devanagari",
        "Egyptian_Hieroglyphs",
        "Ethiopic",
        "Georgian",
        "Glagolitic",
        "Gothic",
        "Greek",
        "Gujarati",
        "Gurmukhi",
        "Han",
        "Hangul",
        "Hanunoo",
        "Hebrew",
        "Hiragana",
        "Imperial_Aramaic",
        "Inherited",
        "Inscriptional_Pahlavi",
        "Inscriptional_Parthian",
        "Javanese",
        "Kaithi",
        "Kannada",
        "Katakana",
        "Kayah_Li",
        "Kharoshthi",
        "Khmer",
        "Lao",
        "Latin",
        "Lepcha",
        "Limbu",
        "Linear_B",
        "Lisu",
        "Lycian",
        "Lydian",
        "Malayalam",
        "Mandaic",
        "Meetei_Mayek",
        "Mongolian",
        "Myanmar",
        "New_Tai_Lue",
        "Nko",
        "Ogham",
        "Ol_Chiki",
        "Old_Italic",
        "Old_Persian",
        "Old_South_Arabian",
        "Old_Turkic",
        "Oriya",
        "Osmanya",
        "Phags_Pa",
        "Phoenician",
        "Rejang",
        "Runic",
        "Samaritan",
        "Saurashtra",
        "Shavian",
        "Sinhala",
        "Sundanese",
        "Syloti_Nagri",
        "Syriac",
        "Tagalog",
        "Tagbanwa",
        "Tai_Le",
        "Tai_Tham",
        "Tai_Viet",
        "Tamil",
        "Telugu",
        "Thaana",
        "Thai",
        "Tibetan",
        "Tifinagh",
        "Ugaritic",
        "Vai",
        "Yi",
    ];
    // Общие категории Unicode в Go (`unicode.Categories`).
    const GO_CATEGORIES: &[&str] = &[
        "C", "Cc", "Cf", "Co", "Cs", "L", "Ll", "Lm", "Lo", "Lt", "Lu", "M", "Mc", "Me", "Mn", "N",
        "Nd", "Nl", "No", "P", "Pc", "Pd", "Pe", "Pf", "Pi", "Po", "Ps", "S", "Sc", "Sk", "Sm",
        "So", "Z", "Zl", "Zp", "Zs",
    ];
    match kind {
        ClassUnicodeKind::OneLetter(c) => GO_CATEGORIES.contains(&c.to_string().as_str()),
        ClassUnicodeKind::Named(name) => {
            GO_CATEGORIES.contains(&name.as_str()) || GO_NAMES.contains(&name.as_str())
        }
        ClassUnicodeKind::NamedValue { .. } => false,
    }
}

fn go_class_set(set: &regex_syntax::ast::ClassSet) -> bool {
    use regex_syntax::ast::{ClassSet, ClassSetItem};
    fn item(it: &ClassSetItem) -> bool {
        match it {
            ClassSetItem::Unicode(class) => go_unicode_class(&class.kind),
            ClassSetItem::Bracketed(inner) => go_class_set(&inner.kind),
            ClassSetItem::Union(union) => union.items.iter().all(item),
            ClassSetItem::Literal(literal) => go_literal(literal),
            ClassSetItem::Range(range) => go_literal(&range.start) && go_literal(&range.end),
            _ => true,
        }
    }
    match set {
        ClassSet::Item(it) => item(it),
        ClassSet::BinaryOp(_) => false,
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
                ]}},
                // JS-style \u escapes: Go's regexp rejects them although Rust accepts.
                "ascii": {"type": "string", "pattern": r"^[\u0000-\u007F]*$"},
                "word": {"type": "string", "pattern": r"^\w{1,500}$"},
                // Data, not schema: left as is.
                "obj": {"type": "object", "default": {"pattern": "(?=x)"}},
                "map": {"type": "object", "patternProperties": {
                    "^[a-z]+$": {"type": "string"},
                    "^(?!x).*$": {"type": "string"}
                }}
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
        assert!(props["ascii"].get("pattern").is_none());
        assert_eq!(
            props["word"]["pattern"], r"^\w{1,500}$",
            "big but valid stays"
        );
        assert_eq!(props["obj"]["default"]["pattern"], "(?=x)");
        let pattern_props = props["map"]["patternProperties"].as_object().unwrap();
        assert!(pattern_props.contains_key("^[a-z]+$"));
        assert!(!pattern_props.contains_key("^(?!x).*$"));
    }

    #[test]
    fn regexes_follow_go_limits() {
        for ok in [
            r"^[a-z0-9-]+$",
            r"^\d{4}-\d{2}-\d{2}$",
            r"(?i)^[\pL\p{Lu}\p{Greek}]+$",
            r"^\x{41}+$",
            r"^[[:alpha:]]{1,1000}$",
            r"^(?:a+)+$",
            r"(?P<year>\d{4})",
            r"^C:\\users\\[a-z]+$",
            r"^[\p{Ethiopic}\p{Thaana}\p{Mongolian}\p{Any}]+$",
        ] {
            assert!(portable_regex(ok), "{ok}");
        }
        for bad in [
            r"^[^\uD800-\uDFFF]*$",
            r"^[\u0000-\u007F]*$",
            r"^\w{1,2000}$",
            r".{1,1001}",
            r"(?:\w{1,100}){50}",
            r"a++",
            r"a**",
            r"(?x)a b",
            r"(?u)\w",
            r"\p{Alphabetic}",
            r"\p{Emoji}",
            r"\p{sc=Latin}",
            r"[\w&&\d]",
            r"\b{start}x",
            r"^(?=a)a$",
            r"\U0001F600",
            r"[\U{1F600}]",
            r"[a-\U0001F600]",
            r"\p{DI}",
            r"\pX",
            r"(?P<a.b>x)",
            r"(?P<имя>x)",
            r"^\u0041$",
            r"\u{41}",
            r"\p{Adlam}",
            r"\p{greek}",
        ] {
            assert!(!portable_regex(bad), "{bad}");
        }
    }

    #[test]
    fn keywords_only_count_inside_a_schema() {
        let schema = json!({"type": "object", "properties": {
            // Properties called like keywords are schemas and get cleaned too.
            "default": {"type": "string", "pattern": "(?=x)"},
            "patternProperties": {"type": "string", "pattern": "(?=y)"}
        }});
        let out = portable_schema(&schema);
        assert!(out["properties"]["default"].get("pattern").is_none());
        assert!(
            out["properties"]["patternProperties"]
                .get("pattern")
                .is_none()
        );
    }

    fn thinking_history(tools: bool) -> Value {
        let mut body = json!({
            "model": "m", "max_tokens": 10,
            "messages": [
                {"role": "user", "content": "go"},
                {"role": "assistant", "content": [
                    {"type": "thinking", "thinking": "plan A", "signature": ""},
                    {"type": "thinking", "thinking": "", "signature": ""},
                    {"type": "thinking", "thinking": "then B", "signature": ""},
                    {"type": "text", "text": "calling"},
                    {"type": "tool_use", "id": "toolu_1", "name": "read", "input": {}}
                ]},
                {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_1", "content": "ok"}]}
            ]
        });
        if tools {
            body["tools"] =
                json!([{"name": "read", "description": "r", "input_schema": {"type": "object"}}]);
        }
        body
    }

    #[test]
    fn the_reasoning_of_an_earlier_reply_goes_back_to_the_provider_when_tools_are_on() {
        // DeepSeek (thinking mode): with `tools` in the request the earlier reasoning_content
        // must come back, or the answer is a 400. Claude Code sends the thinking blocks back.
        let payload = build(&provider(), &thinking_history(true), false);
        let assistant = payload["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["role"] == "assistant")
            .unwrap();
        assert_eq!(assistant["reasoning_content"], "plan A\n\nthen B");
        assert_eq!(assistant["content"], "calling");
        // Without tools DeepSeek ignores the field and other providers may refuse it: not sent.
        let plain = build(&provider(), &thinking_history(false), false);
        assert!(!plain.to_string().contains("reasoning_content"), "{plain}");
    }

    #[test]
    fn a_reply_without_thinking_gets_no_reasoning_field() {
        let mut body = thinking_history(true);
        body["messages"][1]["content"] = json!([
            {"type": "text", "text": "calling"},
            {"type": "tool_use", "id": "toolu_1", "name": "read", "input": {}}
        ]);
        let payload = build(&provider(), &body, false);
        assert!(
            !payload.to_string().contains("reasoning_content"),
            "{payload}"
        );
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
    fn tool_search_results_and_pdfs_say_what_they_are() {
        let result = json!([
            {"type": "tool_reference", "tool_name": "mcp__github__create_issue"},
            {"type": "document", "source": {"type": "base64", "media_type": "application/pdf", "data": "JVBERi0="}}
        ]);
        assert_eq!(
            flatten_content(&result),
            format!("Tool loaded: mcp__github__create_issue\n\n{PDF_OMITTED}")
        );
    }

    #[test]
    fn images_are_replaced_by_one_note_per_run() {
        let mut payload = json!({"messages": [
            {"role": "system", "content": "sys"},
            {"role": "user", "content": [
                {"type": "text", "text": "look"},
                {"type": "image_url", "image_url": {"url": "data:image/png;base64,AAAA"}},
                {"type": "image_url", "image_url": {"url": "data:image/png;base64,BBBB"}},
                {"type": "text", "text": "and tell me"}
            ]},
            {"role": "user", "content": "no images here"}
        ]});
        assert!(has_images(&payload));
        assert_eq!(strip_images(&mut payload), 2);
        assert!(!has_images(&payload));
        assert_eq!(
            payload["messages"][1]["content"],
            "look\n\n[image omitted: this model does not accept images]\n\nand tell me"
        );
        assert_eq!(payload["messages"][2]["content"], "no images here");
        assert_eq!(strip_images(&mut payload), 0);
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
