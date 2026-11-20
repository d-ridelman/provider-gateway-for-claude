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
/// Подтверждения складываются, только если между ними не больше суток.
const PENDING_TTL: Duration = Duration::from_secs(24 * 3600);
/// Предел инструментов у строгих провайдеров (OpenAI: 128).
const TOOL_CAP: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    /// Картинки заменяются пометкой в тексте.
    Images,
    /// Уровень рассуждения (`reasoning_effort`) убирается.
    Effort,
    /// Параметры выборки (`temperature`, `top_p`, `stop`, `parallel_tool_calls`) убираются.
    Sampling,
    /// `stream_options` (учёт токенов в потоке) убирается.
    StreamOptions,
    /// Имена инструментов становятся короткими и из `[A-Za-z0-9_-]`.
    ToolNames,
    /// Подряд идущие сообщения одной роли сливаются.
    MergeRoles,
    /// Схемы инструментов упрощаются до типов и описаний.
    Schemas,
    /// Инструментов не больше 128: хвост (обычно MCP) отбрасывается.
    ToolCap,
}

/// Порядок проб: от самого вероятного и безобидного к самому грубому. Поля убираются по
/// одному виду: выучится только то, что помогло, а не всё сразу.
pub const LADDER: [Step; 8] = [
    Step::Images,
    Step::Effort,
    Step::Sampling,
    Step::StreamOptions,
    Step::ToolNames,
    Step::MergeRoles,
    Step::Schemas,
    Step::ToolCap,
];

impl Step {
    pub fn label(self) -> &'static str {
        match self {
            Self::Images => "images left out",
            Self::Effort => "reasoning effort left out",
            Self::Sampling => "sampling parameters left out",
            Self::StreamOptions => "stream options left out",
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
        Step::Effort => drop_fields(payload, provider, &["reasoning_effort"]),
        Step::Sampling => drop_fields(
            payload,
            provider,
            &["temperature", "top_p", "stop", "parallel_tool_calls"],
        ),
        Step::StreamOptions => drop_fields(payload, provider, &["stream_options"]),
        Step::ToolNames => rename_tools(payload),
        Step::MergeRoles => merge_roles(payload),
        Step::Schemas => simplify_tools(payload),
        Step::ToolCap => cap_tools(payload),
    }
}

/// Поля, без которых запрос остаётся запросом. Заданное самим человеком в `extra_body`
/// провайдера не трогается.
fn drop_fields(payload: &mut Value, provider: &Provider, keys: &[&str]) -> bool {
    let Some(map) = payload.as_object_mut() else {
        return false;
    };
    let mut changed = false;
    for key in keys {
        if !provider.extra_body.contains_key(*key) && map.remove(*key).is_some() {
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
    let hash = fnv(name);
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
/// `anyOf`/`oneOf`/`allOf` дают первый вариант, не `null`; `type: [a, "null"]` — `a`;
/// `enum` без `type` берёт тип по первому значению.
// shortcut: `$ref` без разворота сводится к строке, если нет `properties`; разворачивать `$defs`, если такой провайдер и такие схемы встретятся.
fn simplify_schema(schema: &Value) -> Value {
    let Value::Object(map) = schema else {
        return schema.clone();
    };
    for key in ["anyOf", "oneOf", "allOf"] {
        if let Some(Value::Array(variants)) = map.get(key)
            && let Some(first) = variants
                .iter()
                .find(|variant| variant.get("type").and_then(Value::as_str) != Some("null"))
        {
            let mut out = simplify_schema(first);
            if let (Some(description), Some(object)) = (map.get("description"), out.as_object_mut())
            {
                object.insert("description".into(), description.clone());
            }
            return out;
        }
    }
    let mut out = Map::new();
    match map.get("type") {
        Some(Value::Array(kinds)) => {
            if let Some(kind) = kinds
                .iter()
                .find(|kind| kind.as_str().is_some_and(|k| k != "null"))
            {
                out.insert("type".into(), kind.clone());
            }
        }
        Some(kind) => {
            out.insert("type".into(), kind.clone());
        }
        None => {}
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
        let from_enum = match out.get("enum").and_then(|values| values.get(0)) {
            Some(Value::Number(n)) if n.is_i64() || n.is_u64() => "integer",
            Some(Value::Number(_)) => "number",
            Some(Value::Bool(_)) => "boolean",
            _ => "string",
        };
        out.insert(
            "type".into(),
            json!(if out.contains_key("properties") {
                "object"
            } else {
                from_enum
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

/// Отпечатки картинок запроса (для счёта подтверждений: одна и та же картинка в
/// истории — не новое подтверждение).
pub fn image_ids(payload: &Value) -> Vec<u64> {
    let mut ids: Vec<u64> = payload
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|message| message.get("content").and_then(Value::as_array))
        .flatten()
        .filter(|part| part.get("type").and_then(Value::as_str) == Some("image_url"))
        .map(|part| {
            fnv(part
                .pointer("/image_url/url")
                .and_then(Value::as_str)
                .unwrap_or(""))
        })
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

fn fnv(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Сообщение провайдера об ошибке без эха запроса: `error.message`, `msg` из `detail`
/// (FastAPI и vLLM повторяют во вводе всё, что получили, и в нём «context» и «token»
/// есть почти всегда) или просто начало текста.
pub(crate) fn error_message(text: &str) -> String {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return text.chars().take(600).collect();
    };
    if let Some(message) = crate::error::openai_error_message(&value) {
        return message;
    }
    match value.get("detail").or_else(|| value.get("message")) {
        Some(Value::String(message)) => message.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| item.get("msg").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("; "),
        _ => text.chars().take(600).collect(),
    }
}

/// Причина 400 может быть упрощением, а не запросом по существу? Переполнение окна,
/// лимиты, деньги, ключи, фильтры контента и несуществующая модель лесенку не лечат.
pub fn adaptable(status: u16, text: &str) -> bool {
    if !matches!(status, 400 | 422) {
        return false;
    }
    let message = error_message(text);
    if is_overflow(text) {
        return false;
    }
    let lower = message.to_ascii_lowercase();
    let missing_model = lower.contains("model")
        && (lower.contains("not found") || lower.contains("does not exist"));
    !missing_model
        && !rate_wording(&unquoted(&lower))
        && ![
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
            "max_tokens",
            "max_completion_tokens",
        ]
        .iter()
        .any(|marker| lower.contains(marker))
}

/// Текст без того, что в кавычках: имена инструментов и полей в ошибках схемы
/// (`Invalid schema for function 'mcp__cal__get_daily_events'`) не про частоту.
fn unquoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut quote: Option<char> = None;
    for c in text.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == '\'' || c == '"' => quote = Some(c),
            None => out.push(c),
        }
    }
    out
}

/// Текст про частоту, квоты и суточные лимиты: подождать или показать ошибку, а не сжимать
/// разговор и не упрощать запрос.
fn rate_wording(lower: &str) -> bool {
    [
        "per minute",
        "per day",
        "per hour",
        "monthly",
        "daily",
        "quota",
        "rate limit",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

/// Провайдер говорит, что разговор не влез в окно модели (а не что слишком велик вывод).
pub fn context_overflow(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    // Эти слова о окне, даже если рядом упомянут вывод или «try again» (vLLM: «maximum
    // context length is N tokens. However, you requested M output tokens and your
    // prompt contains…»).
    if [
        "context length",
        "context_length",
        "context window",
        "context size",
        "maximum context",
        "exceeds the context",
        "exceed the context",
        "exceeds the available context",
        "prompt is too long",
        "input is too long",
        "input token count",
        "too many input tokens",
        "range of input length",
        "reduce the length",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
    {
        return true;
    }
    // Остальные формулировки двусмысленны: «too many tokens per minute» — это частота,
    // «max_tokens is too large» — вывод.
    if rate_wording(&lower)
        || [
            "max_tokens",
            "max_completion_tokens",
            "max_new_tokens",
            "output token",
            "completion token",
        ]
        .iter()
        .any(|marker| lower.contains(marker))
    {
        return false;
    }
    ["token limit", "tokens exceed", "too many tokens"]
        .iter()
        .any(|marker| lower.contains(marker))
}

/// Переполнение по сообщению и по коду ошибки в теле: у одних провайдеров текст
/// невнятный, а `code` — `context_length_exceeded` — ясный.
pub fn is_overflow(text: &str) -> bool {
    if context_overflow(&error_message(text)) {
        return true;
    }
    serde_json::from_str::<Value>(text).is_ok_and(|value| {
        ["code", "type"].iter().any(|field| {
            value
                .pointer(&format!("/error/{field}"))
                .and_then(Value::as_str)
                .is_some_and(|code| code.to_ascii_lowercase().contains("context_length"))
        })
    })
}

/// «prompt is too long: N tokens > M maximum» по тексту провайдера, если числа в нём есть:
/// Claude Code достаёт их регуляркой и по ним решает, насколько сжимать.
pub fn overflow_message(text: &str) -> Option<String> {
    use regex::Regex;
    use std::sync::OnceLock;
    static LIMIT: OnceLock<Regex> = OnceLock::new();
    static ACTUAL: OnceLock<Regex> = OnceLock::new();
    static GEMINI: OnceLock<Regex> = OnceLock::new();
    let limit = LIMIT.get_or_init(|| {
        Regex::new(r"(?i)maximum context (?:length|window|size) (?:is|of) (\d[\d,]*)")
            .expect("limit regex")
    });
    let actual = ACTUAL.get_or_init(|| {
        Regex::new(
            r"(?i)(?:resulted in|requested|you sent|contains?|has) (\d[\d,]*) (?:input )?tokens",
        )
        .expect("actual regex")
    });
    let gemini = GEMINI.get_or_init(|| {
        Regex::new(
            r"(?i)token count \((\d+)\) exceeds the maximum number of tokens allowed \((\d+)\)",
        )
        .expect("gemini regex")
    });
    let number = |re: &Regex| {
        re.captures(text)
            .and_then(|caps| caps[1].replace(',', "").parse::<u64>().ok())
    };
    let (limit, actual) = match gemini.captures(text) {
        Some(caps) => (caps[2].parse::<u64>().ok()?, caps[1].parse::<u64>().ok()?),
        None => (number(limit)?, number(actual)?),
    };
    (actual > limit).then(|| format!("prompt is too long: {actual} tokens > {limit} maximum"))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Learned {
    steps: Vec<Step>,
    /// Секунды Unix: когда выучено.
    at: u64,
}

/// Подтверждения одного кандидата: сколько раз помогло, на каких картинках и когда
/// последний раз (старое подтверждение не складывается с новым).
struct Pending {
    steps: Vec<Step>,
    count: u8,
    seen: Vec<u64>,
    at: Instant,
}

#[derive(Default)]
struct Inner {
    learned: HashMap<String, Learned>,
    pending: HashMap<String, Pending>,
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

    /// Шаги, которые для этой модели применяются заранее. Выученное старше срока
    /// забывается и здесь, а не только при загрузке: приложение живёт в трее неделями.
    pub fn steps(&self, key: &str) -> Vec<Step> {
        let mut inner = self.lock();
        let expired = inner
            .learned
            .get(key)
            .map(|learned| learned.at.saturating_add(EXPIRES.as_secs()) < now());
        match expired {
            Some(true) => {
                inner.learned.remove(key);
                self.save(&inner);
                Vec::new()
            }
            Some(false) => inner.learned[key].steps.clone(),
            None => Vec::new(),
        }
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

    /// Запрос прошёл, когда к известным шагам добавился `flipped`: до него отказ ещё был,
    /// с ним — нет. Запоминается только он (предыдущие шаги лесенки не помогли, и лишнее
    /// навсегда отняло бы у модели то, что она принимает). Тот же новый шаг на втором
    /// запросе — выучено; результат — все шаги модели. `images` — отпечатки убранных
    /// картинок: Claude Code шлёт всю историю каждый раз, и один битый скриншот
    /// отказывал бы на каждом ходе. Подтверждение засчитывается, только если ни одной из
    /// картинок ещё не было среди подтверждений (пусто — шаг не про картинки).
    pub fn record_success(&self, key: &str, flipped: Step, images: &[u64]) -> Option<Vec<Step>> {
        let mut inner = self.lock();
        let known: Vec<Step> = inner
            .learned
            .get(key)
            .map(|learned| learned.steps.clone())
            .unwrap_or_default();
        if known.contains(&flipped) {
            return None;
        }
        let candidate: Vec<Step> = LADDER
            .iter()
            .copied()
            .filter(|step| *step == flipped || known.contains(step))
            .collect();
        let confirmed = {
            let entry = inner
                .pending
                .entry(key.to_string())
                .or_insert_with(|| Pending {
                    steps: Vec::new(),
                    count: 0,
                    seen: Vec::new(),
                    at: Instant::now(),
                });
            if entry.steps != candidate || entry.at.elapsed() > PENDING_TTL {
                *entry = Pending {
                    steps: candidate,
                    count: 0,
                    seen: Vec::new(),
                    at: Instant::now(),
                };
            }
            // Только набор, целиком состоящий из невиденных картинок: пока в истории
            // остаётся прежняя битая, отказ полностью объясняется ею, а не новыми.
            let fresh = images.is_empty() || images.iter().all(|id| !entry.seen.contains(id));
            if fresh {
                entry.count += 1;
                entry.seen.extend_from_slice(images);
                entry.at = Instant::now();
            }
            (fresh && entry.count >= CONFIRMATIONS).then(|| entry.steps.clone())
        };
        let steps = confirmed?;
        inner.pending.remove(key);
        inner.learned.insert(
            key.to_string(),
            Learned {
                steps: steps.clone(),
                at: now(),
            },
        );
        self.save(&inner);
        Some(steps)
    }

    /// Запрос с картинками прошёл как есть: картинки модели не мешают, накопленные
    /// «подтверждения» про них были случайностью.
    pub fn images_are_fine(&self, key: &str) {
        let mut inner = self.lock();
        if inner
            .pending
            .get(key)
            .is_some_and(|pending| pending.steps.contains(&Step::Images))
        {
            inner.pending.remove(key);
        }
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
        // Каждый шаг трогает только свои поля: выучится то, что помогло, не всё сразу.
        assert!(apply(Step::Effort, &mut payload, &owner));
        assert!(payload.get("reasoning_effort").is_none());
        assert!(payload.get("stream_options").is_some());
        assert!(apply(Step::StreamOptions, &mut payload, &owner));
        assert!(payload.get("stream_options").is_none());
        assert!(apply(Step::Sampling, &mut payload, &owner));
        assert!(payload.get("parallel_tool_calls").is_none());
        assert_eq!(payload["top_p"], 1);
        assert!(!apply(Step::Sampling, &mut payload, &owner));
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
    fn simplified_schemas_stay_valid_for_unions_and_numeric_enums() {
        let object = json!({"type": "object", "properties": {"x": {"type": "integer"}}});
        let simple = simplify_schema(&json!({
            "properties": {
                "count": {"enum": [1, 2, 3]},
                "flag": {"enum": [true]},
                "maybe": {"type": ["string", "null"], "description": "nullable"},
                "either": {"anyOf": [{"type": "null"}, {"type": "array", "items": {"type": "string"}}],
                           "description": "list or nothing"},
                "shape": {"oneOf": [object.clone(), {"type": "string"}]}
            }
        }));
        assert_eq!(simple["type"], "object");
        let props = &simple["properties"];
        assert_eq!(props["count"]["type"], "integer");
        assert_eq!(props["flag"]["type"], "boolean");
        assert_eq!(
            props["maybe"],
            json!({"type": "string", "description": "nullable"})
        );
        assert_eq!(props["either"]["type"], "array");
        assert_eq!(props["either"]["items"], json!({"type": "string"}));
        assert_eq!(props["either"]["description"], "list or nothing");
        // Объект из `oneOf` остаётся объектом, а не строкой.
        assert_eq!(props["shape"]["type"], "object");
        assert_eq!(
            props["shape"]["properties"]["x"],
            json!({"type": "integer"})
        );
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
        assert!(!adaptable(400, "model glm-9 does not exist"));
        assert!(context_overflow("maximum context length is 8192 tokens"));
        assert!(context_overflow("Prompt is too long: 300000 tokens"));
        assert!(!context_overflow("the request is invalid"));
        // Предел вывода — не переполнение окна, но и не капризы запроса.
        assert!(!context_overflow(
            "max_tokens exceeds the model's output token limit"
        ));
        assert!(!adaptable(
            400,
            r#"{"error":{"message":"max_tokens is too large: 99999"}}"#
        ));
        // Внятные отказы, которые лечат шаги лесенки.
        let long_name = r#"{"error":{"message":"Invalid 'tools[3].function.name': string too long. Expected a string with maximum length 64"}}"#;
        assert!(adaptable(400, long_name));
        assert!(adaptable(
            400,
            r#"{"error":{"message":"Unexpected token in schema"}}"#
        ));
        assert!(adaptable(
            400,
            r#"{"error":{"message":"array too long: tools"}}"#
        ));
        // Эхо запроса в 422 (FastAPI, vLLM) не должно принимать за переполнение или лимит.
        let echo = r#"{"detail":[{"type":"extra_forbidden","loc":["body","reasoning_effort"],"msg":"Extra inputs are not permitted","input":{"messages":[{"content":"the context and the token budget"}]}}]}"#;
        assert!(adaptable(422, echo));
        assert!(!adaptable(
            422,
            r#"{"detail":[{"msg":"maximum context length is 8192 tokens"}]}"#
        ));
    }

    #[test]
    fn overflow_numbers_are_taken_from_the_providers_words() {
        let openai = "This model's maximum context length is 8192 tokens. However, your messages resulted in 9,000 tokens. Please reduce the length of the messages.";
        assert_eq!(
            overflow_message(openai).as_deref(),
            Some("prompt is too long: 9000 tokens > 8192 maximum")
        );
        assert_eq!(
            overflow_message("maximum context length is 8192 tokens"),
            None
        );
        // Число не больше предела — это не переполнение.
        assert_eq!(
            overflow_message("maximum context length is 8192 tokens, requested 100 tokens"),
            None
        );
    }

    #[test]
    fn an_old_lesson_is_forgotten_in_memory_too() {
        let profiles = Profiles::in_memory();
        profiles.lock().learned.insert(
            "p/old".into(),
            Learned {
                steps: vec![Step::Images],
                at: 1,
            },
        );
        profiles.lock().learned.insert(
            "p/fresh".into(),
            Learned {
                steps: vec![Step::Images],
                at: now(),
            },
        );
        assert!(profiles.steps("p/old").is_empty());
        assert_eq!(profiles.steps("p/fresh"), vec![Step::Images]);
        assert_eq!(profiles.snapshot().len(), 1);
    }

    #[test]
    fn the_same_broken_image_in_the_history_is_one_confirmation_not_two() {
        let profiles = Profiles::in_memory();
        // Ход 1: картинка 7 отказывает. Ход 2: она же в истории, новых нет.
        assert_eq!(profiles.record_success("p/m", Step::Images, &[7]), None);
        assert_eq!(profiles.record_success("p/m", Step::Images, &[7]), None);
        assert!(profiles.steps("p/m").is_empty());
        // Новый скриншот рядом с прежним битым ничего не доказывает: отказ объясняется 7.
        assert_eq!(profiles.record_success("p/m", Step::Images, &[7, 8]), None);
        // А запрос, где все картинки новые (другая сессия), — второе подтверждение.
        assert_eq!(
            profiles.record_success("p/m", Step::Images, &[9]),
            Some(vec![Step::Images])
        );
        // Запрос с картинками, прошедший как есть, обнуляет накопленное.
        let other = Profiles::in_memory();
        assert_eq!(other.record_success("p/m", Step::Images, &[1]), None);
        other.images_are_fine("p/m");
        assert_eq!(other.record_success("p/m", Step::Images, &[2]), None);
        assert!(other.steps("p/m").is_empty());
    }

    #[test]
    fn overflow_wordings_of_other_providers_are_recognised() {
        for text in [
            "the request exceeds the available context size, try increasing it",
            "The input token count (1196265) exceeds the maximum number of tokens allowed (1048575).",
            "exceeded model token limit: 8192",
            "This model's maximum context length is 32768 tokens. However, you requested 2000 output tokens and your prompt contains at least 30769 input tokens",
        ] {
            assert!(context_overflow(text), "{text}");
            assert!(is_overflow(
                &json!({"error": {"message": text}}).to_string()
            ));
            assert!(!adaptable(
                400,
                &json!({"error": {"message": text}}).to_string()
            ));
        }
        // Код в теле при невнятном тексте.
        let coded = r#"{"error":{"message":"Bad Request","code":"context_length_exceeded"}}"#;
        assert!(is_overflow(coded) && !adaptable(400, coded));
        // Квоты и частота — не окно: Claude Code не должен из-за них сжимать разговор.
        for text in [
            "monthly token limit reached",
            "Too many tokens per minute, please wait",
            "Requested output exceeds the maximum number of tokens allowed for output",
            "Input length validation failed for field name",
        ] {
            assert!(!context_overflow(text), "{text}");
        }
        // Частота и квоты в 400/422 лесенку тоже не запускают (иначе до 8 повторов
        // в лимит и случайное «выучивание»).
        for text in [
            "monthly token limit reached",
            "Too many tokens per minute, please wait",
            "daily request limit exceeded",
        ] {
            let body = json!({"error": {"message": text}}).to_string();
            assert!(!adaptable(400, &body) && !adaptable(422, &body), "{text}");
        }
        // Сильные слова про окно сильнее «try again», «daily» и «quota» в том же тексте.
        for text in [
            "maximum context length is 8192 tokens, reduce the length and try again",
            "maximum context length is 8192 tokens; your daily quota is fine",
            "Range of input length should be [1, 30720]",
            // Двусмысленные слова без rate-слов — тоже окно.
            "Too many tokens in the request",
            "total tokens exceed the model limit",
        ] {
            assert!(context_overflow(text), "{text}");
        }
        // Имя инструмента с «daily» в ошибке схемы — не про частоту: лесенка нужна.
        let schema = json!({"error": {"message":
            "Invalid schema for function 'mcp__cal__get_daily_events': 'additionalProperties' is required"}})
        .to_string();
        assert!(adaptable(400, &schema));
        // Лимит вывода — не окно.
        assert!(!context_overflow(
            "max_tokens is too large: 100000 for this model"
        ));
        assert_eq!(
            overflow_message(
                "The input token count (1196265) exceeds the maximum number of tokens allowed (1048575)."
            )
            .as_deref(),
            Some("prompt is too long: 1196265 tokens > 1048575 maximum")
        );
        assert_eq!(
            overflow_message(
                "maximum context length of 8192 tokens, but your messages resulted in 9000 tokens"
            )
            .as_deref(),
            Some("prompt is too long: 9000 tokens > 8192 maximum")
        );
        assert_eq!(
            overflow_message(
                "maximum context length is 32768 tokens. However, you requested 2000 output tokens and your prompt contains at least 30769 input tokens"
            ),
            None,
            "30769 < 32768 requested+prompt: no honest numbers, the text goes as is"
        );
    }

    #[test]
    fn a_model_is_learned_on_the_second_confirmation_and_kept_on_disk() {
        let dir = std::env::temp_dir().join(format!("ctp-profiles-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("model-profiles.json");

        let profiles = Profiles::load(path.clone());
        // Запоминается шаг, который перевёл отказ в успех, и только после второго раза.
        assert_eq!(profiles.record_success("p/m", Step::Images, &[1]), None);
        assert!(profiles.steps("p/m").is_empty());
        // Другой шаг — счёт заново.
        assert_eq!(profiles.record_success("p/m", Step::Effort, &[]), None);
        assert_eq!(
            profiles.record_success("p/m", Step::Effort, &[]),
            Some(vec![Step::Effort])
        );
        assert_eq!(profiles.steps("p/m"), vec![Step::Effort]);
        // Уже известный шаг ничего не меняет.
        assert_eq!(profiles.record_success("p/m", Step::Effort, &[]), None);
        // Выученное дополняется тоже со второго раза; порядок — как в лесенке.
        assert_eq!(profiles.record_success("p/m", Step::Images, &[2]), None);
        assert_eq!(
            profiles.record_success("p/m", Step::Images, &[3]),
            Some(vec![Step::Images, Step::Effort])
        );

        // После перезапуска — то же.
        let reloaded = Profiles::load(path.clone());
        assert_eq!(reloaded.steps("p/m"), vec![Step::Images, Step::Effort]);
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
