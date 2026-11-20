//! Приспособление запроса к капризам провайдера. Claude Code шлёт запросы, на которые
//! часть OpenAI-совместимых моделей отвечает 400 без объяснений (`glm-5.3` — на любую
//! картинку: «the request is invalid»). Здесь три слоя:
//! 1. то, что безопасно всегда (повтор имён инструментов, `request::build`);
//! 2. лесенка упрощений на неясный 400: каждый шаг пробуется отдельно, поверх
//!    предыдущих; помог — запрос идёт дальше;
//! 3. профиль модели: помогало дважды — шаги применяются заранее и хранятся на диске.

use crate::state::Provider;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Сколько раз упрощение должно помочь на разных запросах, прежде чем оно запомнится.
/// Один случай — может быть и разовая причина (скриншот не по формату, переполнение).
const CONFIRMATIONS: u8 = 2;
/// Выученное забывается: провайдер мог научиться принимать картинки.
const EXPIRES: Duration = Duration::from_secs(30 * 24 * 3600);
/// Лесенка не помогла: следующие 400 этой модели не гоняем по ней, это не капризы.
const COOLDOWN: Duration = Duration::from_secs(5 * 60);
/// Предел инструментов у строгих провайдеров (OpenAI: 128).
const TOOL_CAP: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    /// Картинки заменяются пометкой в тексте.
    Images,
    /// Необязательные поля запроса убираются.
    Fields,
    /// Имена инструментов становятся короткими и из `[A-Za-z0-9_-]`.
    ToolNames,
    /// Подряд идущие сообщения одной роли сливаются.
    MergeRoles,
    /// Схемы инструментов упрощаются до типов и описаний.
    Schemas,
    /// Инструментов не больше 128: хвост (обычно MCP) отбрасывается.
    ToolCap,
}

/// Порядок проб: от самого вероятного и безобидного к самому грубому.
pub const LADDER: [Step; 6] = [
    Step::Images,
    Step::Fields,
    Step::ToolNames,
    Step::MergeRoles,
    Step::Schemas,
    Step::ToolCap,
];

impl Step {
    pub fn label(self) -> &'static str {
        match self {
            Self::Images => "images left out",
            Self::Fields => "optional request fields left out",
            Self::ToolNames => "tool names shortened",
            Self::MergeRoles => "consecutive messages merged",
            Self::Schemas => "tool schemas simplified",
            Self::ToolCap => "tools capped at 128",
        }
    }
}

/// Применить шаг к запросу провайдеру. `true` — запрос изменился (иначе повторять нечего).
pub fn apply(step: Step, payload: &mut Value, provider: &Provider) -> bool {
    match step {
        Step::Images => crate::openai::request::strip_images(payload) > 0,
        Step::Fields => drop_fields(payload, provider),
        Step::ToolNames => rename_tools(payload),
        Step::MergeRoles => merge_roles(payload),
        Step::Schemas => simplify_tools(payload),
        Step::ToolCap => cap_tools(payload),
    }
}

/// Поля, без которых запрос остаётся запросом. Заданное самим человеком в `extra_body`
/// провайдера не трогается.
fn drop_fields(payload: &mut Value, provider: &Provider) -> bool {
    let Some(map) = payload.as_object_mut() else {
        return false;
    };
    let mut changed = false;
    for key in [
        "reasoning_effort",
        "parallel_tool_calls",
        "stream_options",
        "temperature",
        "top_p",
        "stop",
    ] {
        if !provider.extra_body.contains_key(key) && map.remove(key).is_some() {
            changed = true;
        }
    }
    changed
}

fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Безопасное имя инструмента: то же, если подходит, иначе `[A-Za-z0-9_-]`, не длиннее 64,
/// с отпечатком оригинала (два длинных имени с общим началом не склеятся).
pub fn safe_name(name: &str) -> String {
    if is_valid_name(name) {
        return name.to_string();
    }
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in name.bytes() {
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
    }
    let head: String = cleaned.chars().take(54).collect();
    format!("{head}_{:08x}", hash & 0xffff_ffff)
}

/// Безопасное имя → настоящее: для имён инструментов запроса, которые пришлось бы менять.
/// Нужна ответу: модель зовёт инструмент по безопасному имени, Claude Code знает настоящее.
pub fn name_map(body: &Value) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let mut note = |name: &str| {
        let safe = safe_name(name);
        if safe != name {
            map.insert(safe, name.to_string());
        }
    };
    for tool in body
        .get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(name) = tool.get("name").and_then(Value::as_str) {
            note(name);
        }
    }
    for message in body
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        for block in message
            .get("content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if block.get("type").and_then(Value::as_str) == Some("tool_use")
                && let Some(name) = block.get("name").and_then(Value::as_str)
            {
                note(name);
            }
        }
    }
    map
}

fn rename_tools(payload: &mut Value) -> bool {
    let mut changed = false;
    let mut rename = |name: &mut Value| {
        if let Some(old) = name.as_str() {
            let safe = safe_name(old);
            if safe != old {
                *name = Value::String(safe);
                changed = true;
            }
        }
    };
    for tool in payload
        .get_mut("tools")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        if let Some(name) = tool.pointer_mut("/function/name") {
            rename(name);
        }
    }
    for message in payload
        .get_mut("messages")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        for call in message
            .get_mut("tool_calls")
            .and_then(Value::as_array_mut)
            .into_iter()
            .flatten()
        {
            if let Some(name) = call.pointer_mut("/function/name") {
                rename(name);
            }
        }
    }
    if let Some(name) = payload.pointer_mut("/tool_choice/function/name") {
        rename(name);
    }
    changed
}

/// Подряд идущие `user` (и `assistant` без вызовов) со строковым содержимым — одним.
fn merge_roles(payload: &mut Value) -> bool {
    let Some(messages) = payload.get_mut("messages").and_then(Value::as_array_mut) else {
        return false;
    };
    let mergeable = |message: &Value| {
        let role = message.get("role").and_then(Value::as_str);
        message.get("content").is_some_and(Value::is_string)
            && message.get("tool_calls").is_none()
            && matches!(role, Some("user" | "assistant"))
    };
    let mut merged: Vec<Value> = Vec::with_capacity(messages.len());
    let mut changed = false;
    for message in messages.drain(..) {
        if let Some(last) = merged.last_mut()
            && mergeable(last)
            && mergeable(&message)
            && last.get("role") == message.get("role")
        {
            let text = format!(
                "{}\n\n{}",
                last["content"].as_str().unwrap_or(""),
                message["content"].as_str().unwrap_or("")
            );
            last["content"] = Value::String(text);
            changed = true;
        } else {
            merged.push(message);
        }
    }
    *messages = merged;
    changed
}

/// Схема только из типов, описаний, перечислений и вложенности: `anyOf`, `$ref`, форматы,
/// пределы и прочее, что строгий провайдер может не понять, уходит. Описание остаётся и
/// ведёт модель.
// shortcut: `anyOf`/`oneOf` без `type` сводятся к строке; точнее — выбирать первый вариант, если такой провайдер встретится.
fn simplify_schema(schema: &Value) -> Value {
    let Value::Object(map) = schema else {
        return schema.clone();
    };
    let mut out = Map::new();
    if let Some(kind) = map.get("type") {
        out.insert("type".into(), kind.clone());
    }
    if let Some(Value::String(text)) = map.get("description") {
        out.insert("description".into(), json!(text));
    }
    if let Some(values) = map.get("enum") {
        out.insert("enum".into(), values.clone());
    }
    if let Some(Value::Object(props)) = map.get("properties") {
        out.insert(
            "properties".into(),
            Value::Object(
                props
                    .iter()
                    .map(|(name, sub)| (name.clone(), simplify_schema(sub)))
                    .collect(),
            ),
        );
    }
    if let Some(items) = map.get("items") {
        out.insert("items".into(), simplify_schema(items));
    }
    if let Some(Value::Array(required)) = map.get("required") {
        out.insert("required".into(), Value::Array(required.clone()));
    }
    if !out.contains_key("type") {
        out.insert(
            "type".into(),
            json!(if out.contains_key("properties") {
                "object"
            } else {
                "string"
            }),
        );
    }
    Value::Object(out)
}

fn simplify_tools(payload: &mut Value) -> bool {
    let mut changed = false;
    for tool in payload
        .get_mut("tools")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        if let Some(parameters) = tool.pointer_mut("/function/parameters") {
            let simple = simplify_schema(parameters);
            if simple != *parameters {
                *parameters = simple;
                changed = true;
            }
        }
    }
    changed
}

/// Больше предела — первые `TOOL_CAP`; функция советника шлюза остаётся всегда.
fn cap_tools(payload: &mut Value) -> bool {
    let Some(tools) = payload.get_mut("tools").and_then(Value::as_array_mut) else {
        return false;
    };
    if tools.len() <= TOOL_CAP {
        return false;
    }
    let is_advisor = |tool: &Value| {
        tool.pointer("/function/name").and_then(Value::as_str)
            == Some(crate::openai::request::ADVISOR)
    };
    let advisor: Vec<Value> = tools.iter().filter(|t| is_advisor(t)).cloned().collect();
    tools.retain(|tool| !is_advisor(tool));
    tools.truncate(TOOL_CAP - advisor.len());
    tools.extend(advisor);
    true
}

/// Причина 400 может быть упрощением, а не запросом по существу? Переполнение окна,
/// лимиты, деньги, ключи и фильтры контента лесенка не лечит.
pub fn adaptable(status: u16, text: &str) -> bool {
    if !matches!(status, 400 | 422) {
        return false;
    }
    let lower = text.to_ascii_lowercase();
    ![
        "context",
        "too long",
        "too large",
        "token",
        "rate limit",
        "quota",
        "balance",
        "credit",
        "billing",
        "api key",
        "api_key",
        "unauthorized",
        "permission",
        "content_filter",
        "content filter",
        "content policy",
        "moderation",
        "safety",
        "not found",
        "does not exist",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

/// Провайдер говорит, что запрос не влез в окно модели.
pub fn context_overflow(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    [
        "context length",
        "context_length",
        "context window",
        "maximum context",
        "exceeds the context",
        "exceed the context",
        "prompt is too long",
        "input is too long",
        "too many tokens",
        "token limit",
        "tokens exceed",
        "reduce the length",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Learned {
    steps: Vec<Step>,
    /// Секунды Unix: когда выучено.
    at: u64,
}

#[derive(Default)]
struct Inner {
    learned: HashMap<String, Learned>,
    pending: HashMap<String, (Vec<Step>, u8)>,
    cooling: HashMap<String, Instant>,
}

/// Что шлюз уже знает о капризах моделей: в памяти и (если задан файл) на диске.
#[derive(Default)]
pub struct Profiles {
    path: Option<PathBuf>,
    inner: Mutex<Inner>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

impl Profiles {
    pub fn in_memory() -> Self {
        Self::default()
    }

    /// Профили из файла (нет файла или он битый — пустые); устаревшие записи пропускаются.
    pub fn load(path: PathBuf) -> Self {
        let mut inner = Inner::default();
        if let Ok(text) = std::fs::read_to_string(&path)
            && let Ok(value) = serde_json::from_str::<Value>(&text)
            && let Some(models) = value.get("models").and_then(Value::as_object)
        {
            let limit = now().saturating_sub(EXPIRES.as_secs());
            for (key, entry) in models {
                if let Ok(learned) = serde_json::from_value::<Learned>(entry.clone())
                    && learned.at >= limit
                    && !learned.steps.is_empty()
                {
                    inner.learned.insert(key.clone(), learned);
                }
            }
        }
        Self {
            path: Some(path),
            inner: Mutex::new(inner),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|exc| exc.into_inner())
    }

    /// Шаги, которые для этой модели применяются заранее.
    pub fn steps(&self, key: &str) -> Vec<Step> {
        self.lock()
            .learned
            .get(key)
            .map(|learned| learned.steps.clone())
            .unwrap_or_default()
    }

    /// Выученное: `(модель, шаги)` по алфавиту (для журнала и диагностики).
    pub fn snapshot(&self) -> Vec<(String, Vec<Step>)> {
        let mut all: Vec<_> = self
            .lock()
            .learned
            .iter()
            .map(|(key, learned)| (key.clone(), learned.steps.clone()))
            .collect();
        all.sort_by(|a, b| a.0.cmp(&b.0));
        all
    }

    /// Запрос прошёл после упрощений `steps`. Второй такой случай подряд с теми же
    /// шагами — модель выучена (результат — шаги). Выученное дополняется новыми шагами.
    pub fn record_success(&self, key: &str, steps: &[Step]) -> Option<Vec<Step>> {
        if steps.is_empty() {
            return None;
        }
        let mut inner = self.lock();
        let mut ordered: Vec<Step> = LADDER
            .iter()
            .copied()
            .filter(|step| steps.contains(step))
            .collect();
        let result = if let Some(learned) = inner.learned.get_mut(key) {
            let before = learned.steps.len();
            for step in &ordered {
                if !learned.steps.contains(step) {
                    learned.steps.push(*step);
                }
            }
            learned.at = now();
            (learned.steps.len() != before).then(|| learned.steps.clone())
        } else {
            let entry = inner
                .pending
                .entry(key.to_string())
                .or_insert_with(|| (Vec::new(), 0));
            if entry.0 == ordered {
                entry.1 += 1;
            } else {
                *entry = (std::mem::take(&mut ordered), 1);
            }
            if entry.1 >= CONFIRMATIONS {
                let steps = entry.0.clone();
                inner.pending.remove(key);
                inner.learned.insert(
                    key.to_string(),
                    Learned {
                        steps: steps.clone(),
                        at: now(),
                    },
                );
                Some(steps)
            } else {
                None
            }
        };
        if result.is_some() {
            self.save(&inner);
        }
        result
    }

    /// Лесенка этой модели недавно не помогла.
    pub fn cooling(&self, key: &str) -> bool {
        let mut inner = self.lock();
        match inner.cooling.get(key) {
            Some(since) if since.elapsed() < COOLDOWN => true,
            Some(_) => {
                inner.cooling.remove(key);
                false
            }
            None => false,
        }
    }

    pub fn cool(&self, key: &str) {
        self.lock().cooling.insert(key.to_string(), Instant::now());
    }

    fn save(&self, inner: &Inner) {
        let Some(path) = &self.path else { return };
        let models: Map<String, Value> = inner
            .learned
            .iter()
            .filter_map(|(key, learned)| Some((key.clone(), serde_json::to_value(learned).ok()?)))
            .collect();
        let text = serde_json::to_string_pretty(&json!({"version": 1, "models": models}))
            .unwrap_or_default();
        let temp = path.with_extension("json.tmp");
        if std::fs::write(&temp, text).is_ok() && std::fs::rename(&temp, path).is_err() {
            let _ = std::fs::remove_file(&temp);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Provider, ProviderKind};
    use std::collections::BTreeMap;

    fn provider(extra: &[(&str, Value)]) -> Provider {
        Provider {
            id: "p".into(),
            name: "P".into(),
            kind: ProviderKind::OpenAi,
            base_url: "https://p.example/v1".into(),
            api_key: String::new(),
            headers: BTreeMap::new(),
            chat_completions_url: None,
            max_tokens_field: None,
            extra_body: extra
                .iter()
                .map(|(key, value)| (key.to_string(), value.clone()))
                .collect(),
            include_usage: false,
            max_effort: None,
            default_effort: None,
            session_header: None,
        }
    }

    fn tool(name: &str, parameters: Value) -> Value {
        json!({"type": "function", "function": {"name": name, "description": "d", "parameters": parameters}})
    }

    #[test]
    fn fields_go_but_the_owners_extra_body_stays() {
        let mut payload = json!({"model": "m", "messages": [], "reasoning_effort": "high",
            "parallel_tool_calls": true, "stream_options": {"include_usage": true}, "top_p": 1});
        let owner = provider(&[("top_p", json!(0.9))]);
        assert!(drop_fields(&mut payload, &owner));
        assert!(payload.get("reasoning_effort").is_none());
        assert!(payload.get("stream_options").is_none());
        assert_eq!(payload["top_p"], 1);
        assert!(!drop_fields(&mut payload, &owner));
    }

    #[test]
    fn long_and_odd_tool_names_get_safe_ones_that_map_back() {
        let long = format!(
            "mcp__a2894953-72ff-4bc6-b3d8-d5639bd884c8__{}",
            "x".repeat(40)
        );
        let other = format!(
            "mcp__a2894953-72ff-4bc6-b3d8-d5639bd884c8__{}",
            "y".repeat(40)
        );
        let dotted = "mcp.server/tool one";
        let (a, b, c) = (safe_name(&long), safe_name(&other), safe_name(dotted));
        for name in [&a, &b, &c] {
            assert!(is_valid_name(name), "{name}");
        }
        assert_ne!(a, b, "the same prefix must not collide");
        assert_eq!(safe_name("read_file"), "read_file");

        let body = json!({"tools": [{"name": long}, {"name": other}, {"name": dotted}, {"name": "ok"}],
            "messages": [{"role": "assistant", "content": [
                {"type": "tool_use", "id": "t", "name": long, "input": {}}]}]});
        let map = name_map(&body);
        assert_eq!(map.len(), 3);
        assert_eq!(map[&a], long);
        assert_eq!(map[&c], dotted);

        let mut payload = json!({
            "tools": [tool(&long, json!({"type": "object"})), tool("ok", json!({"type": "object"}))],
            "messages": [{"role": "assistant", "content": null, "tool_calls": [
                {"id": "t", "type": "function", "function": {"name": long, "arguments": "{}"}}]}],
            "tool_choice": {"type": "function", "function": {"name": long}}
        });
        assert!(rename_tools(&mut payload));
        assert_eq!(payload["tools"][0]["function"]["name"], a.as_str());
        assert_eq!(payload["tools"][1]["function"]["name"], "ok");
        assert_eq!(
            payload["messages"][0]["tool_calls"][0]["function"]["name"],
            a.as_str()
        );
        assert_eq!(payload["tool_choice"]["function"]["name"], a.as_str());
        assert!(!rename_tools(&mut payload));
    }

    #[test]
    fn consecutive_roles_merge_but_tool_turns_stay() {
        let mut payload = json!({"messages": [
            {"role": "system", "content": "s"},
            {"role": "user", "content": "a"},
            {"role": "user", "content": "b"},
            {"role": "assistant", "content": null, "tool_calls": [{"id": "1"}]},
            {"role": "tool", "tool_call_id": "1", "content": "r"},
            {"role": "assistant", "content": "x"},
            {"role": "assistant", "content": "y"},
        ]});
        assert!(merge_roles(&mut payload));
        let messages = payload["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 5);
        assert_eq!(messages[1]["content"], "a\n\nb");
        assert_eq!(messages[4]["content"], "x\n\ny");
        assert!(!merge_roles(&mut payload));
    }

    #[test]
    fn schemas_keep_types_descriptions_and_nesting_only() {
        let mut payload = json!({"tools": [tool("t", json!({
            "type": "object", "additionalProperties": false, "$schema": "x",
            "properties": {
                "path": {"type": "string", "format": "uri", "minLength": 1, "description": "where"},
                "mode": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "how"},
                "list": {"type": "array", "items": {"type": "integer", "maximum": 5}},
                "kind": {"enum": ["a", "b"]}
            },
            "required": ["path"]
        }))]});
        assert!(simplify_tools(&mut payload));
        let params = &payload["tools"][0]["function"]["parameters"];
        assert!(params.get("additionalProperties").is_none());
        assert_eq!(params["required"], json!(["path"]));
        assert_eq!(
            params["properties"]["path"],
            json!({"type": "string", "description": "where"})
        );
        assert_eq!(
            params["properties"]["mode"],
            json!({"type": "string", "description": "how"})
        );
        assert_eq!(
            params["properties"]["list"]["items"],
            json!({"type": "integer"})
        );
        assert_eq!(params["properties"]["kind"]["enum"], json!(["a", "b"]));
        assert!(!simplify_tools(&mut payload));
    }

    #[test]
    fn the_tool_cap_keeps_the_advisor() {
        let mut tools: Vec<Value> = (0..200)
            .map(|i| tool(&format!("t{i}"), json!({"type": "object"})))
            .collect();
        tools.push(tool("advisor", json!({"type": "object", "properties": {}})));
        let mut payload = json!({"tools": tools});
        assert!(cap_tools(&mut payload));
        let names: Vec<&str> = payload["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["function"]["name"].as_str().unwrap())
            .collect();
        assert_eq!(names.len(), TOOL_CAP);
        assert_eq!(names[0], "t0");
        assert_eq!(*names.last().unwrap(), "advisor");
        assert!(!cap_tools(&mut payload));
    }

    #[test]
    fn only_unexplained_400s_are_adaptable() {
        assert!(adaptable(400, "[invalid_request] the request is invalid"));
        assert!(adaptable(422, "Unprocessable"));
        assert!(!adaptable(401, "bad"));
        assert!(!adaptable(429, "slow down"));
        assert!(!adaptable(
            400,
            "This model's maximum context length is 8192 tokens"
        ));
        assert!(!adaptable(400, "Insufficient balance"));
        assert!(!adaptable(400, "blocked by content policy"));
        assert!(context_overflow("maximum context length is 8192 tokens"));
        assert!(context_overflow("Prompt is too long: 300000 tokens"));
        assert!(!context_overflow("the request is invalid"));
    }

    #[test]
    fn a_model_is_learned_on_the_second_confirmation_and_kept_on_disk() {
        let dir = std::env::temp_dir().join(format!("ctp-profiles-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("model-profiles.json");

        let profiles = Profiles::load(path.clone());
        assert_eq!(profiles.record_success("p/m", &[Step::Images]), None);
        assert!(profiles.steps("p/m").is_empty());
        // Другие шаги — счёт заново.
        assert_eq!(profiles.record_success("p/m", &[Step::Fields]), None);
        assert_eq!(
            profiles.record_success("p/m", &[Step::Fields]),
            Some(vec![Step::Fields])
        );
        assert_eq!(profiles.steps("p/m"), vec![Step::Fields]);
        // Выученное дополняется, порядок — как в лесенке.
        assert_eq!(
            profiles.record_success("p/m", &[Step::Schemas, Step::Fields]),
            Some(vec![Step::Fields, Step::Schemas])
        );
        assert_eq!(profiles.record_success("p/m", &[]), None);

        // После перезапуска — то же.
        let reloaded = Profiles::load(path.clone());
        assert_eq!(reloaded.steps("p/m"), vec![Step::Fields, Step::Schemas]);
        assert_eq!(reloaded.snapshot().len(), 1);

        // Устаревшее забывается; битый файл — пустые профили.
        std::fs::write(
            &path,
            r#"{"version":1,"models":{"p/old":{"steps":["images"],"at":1},"p/new":{"steps":["images"],"at":99999999999}}}"#,
        )
        .unwrap();
        let aged = Profiles::load(path.clone());
        assert!(aged.steps("p/old").is_empty());
        assert_eq!(aged.steps("p/new"), vec![Step::Images]);
        std::fs::write(&path, "{oops").unwrap();
        assert!(Profiles::load(path).snapshot().is_empty());

        // Остужение.
        let cold = Profiles::in_memory();
        assert!(!cold.cooling("p/m"));
        cold.cool("p/m");
        assert!(cold.cooling("p/m"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
