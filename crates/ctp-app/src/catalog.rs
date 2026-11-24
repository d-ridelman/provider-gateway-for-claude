//! Каталог моделей OpenCode (`models.opencode.ai`): имя, контекст и уровни рассуждения
//! для моделей провайдера, найденного по адресу. Только подсказки для формы: сбой каталога
//! (сеть, смена схемы — URL помечен `experimental`) оставляет всё как без него.

use crate::tr;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Read;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const URL: &str = "https://models.opencode.ai/experimental/v2.0/api.json?type=chat";
const TIMEOUT: Duration = Duration::from_secs(20);
const FRESH: Duration = Duration::from_secs(3600);
/// Сбой помним недолго: без сети каждый клик иначе снова ждал бы таймаута.
const RETRY: Duration = Duration::from_secs(300);
/// Распакованный каталог ~11 МБ; больше — что-то не то.
const MAX_BYTES: u64 = 64 * 1024 * 1024;

/// Уровни Claude Desktop, на которые ложатся уровни каталога.
const CLAUDE_LEVELS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Hint {
    pub name: Option<String>,
    pub context: Option<u64>,
    /// Предел вывода модели в токенах.
    pub output: Option<u64>,
    /// Уровни рассуждения модели из числа уровней Claude, по возрастанию.
    pub efforts: Vec<&'static str>,
    pub default_effort: Option<&'static str>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Hints {
    /// Имя провайдера в каталоге (`OpenCode Go`); `None` — провайдер не найден.
    pub source: Option<String>,
    pub models: HashMap<String, Hint>,
}

// Схема каталога живая (`reasoning.default.value` бывает и строкой, и `true`): поле
// неожиданного вида пропускается (`lenient`), а не роняет весь каталог.
#[derive(Deserialize)]
#[serde(transparent)]
pub struct Catalog(HashMap<String, Lenient<CatProvider>>);

/// `Some`, если запись разобралась; иначе `None` — соседние записи это не задевает.
struct Lenient<T>(Option<T>);

impl<'de, T: serde::de::DeserializeOwned> Deserialize<'de> for Lenient<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        Ok(Lenient(serde_json::from_value(value).ok()))
    }
}

fn lenient<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned + Default,
{
    Ok(Lenient::<T>::deserialize(deserializer)?
        .0
        .unwrap_or_default())
}

#[derive(Deserialize)]
struct CatProvider {
    #[serde(default, deserialize_with = "lenient")]
    name: Option<String>,
    #[serde(default)]
    models: HashMap<String, Lenient<CatModel>>,
}

#[derive(Deserialize)]
struct CatModel {
    #[serde(default, deserialize_with = "lenient")]
    name: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    limit: Option<Limit>,
    #[serde(default, deserialize_with = "lenient")]
    capabilities: Option<Capabilities>,
    #[serde(default, deserialize_with = "lenient")]
    api: HashMap<String, Lenient<Api>>,
}

#[derive(Default, Deserialize)]
struct Limit {
    #[serde(default, deserialize_with = "lenient")]
    context: Option<u64>,
    #[serde(default, deserialize_with = "lenient")]
    output: Option<u64>,
}

#[derive(Default, Deserialize)]
struct Capabilities {
    #[serde(default, deserialize_with = "lenient")]
    reasoning: Option<Reasoning>,
}

#[derive(Default, Deserialize)]
struct Reasoning {
    #[serde(default, deserialize_with = "lenient")]
    supported: bool,
    #[serde(default, deserialize_with = "lenient")]
    effort: Vec<Lenient<String>>,
    #[serde(default, deserialize_with = "lenient")]
    default: Option<ReasoningDefault>,
}

#[derive(Default, Deserialize)]
struct ReasoningDefault {
    #[serde(default, deserialize_with = "lenient")]
    value: Option<String>,
}

#[derive(Default, Deserialize)]
struct Api {
    #[serde(default, deserialize_with = "lenient")]
    base_url: Option<String>,
}

/// Последняя загрузка: каталог или `None` после сбоя.
type Cached = (Instant, Option<Arc<Catalog>>);
static CACHE: Mutex<Option<Cached>> = Mutex::new(None);

/// Одна загрузка за раз: повторный клик ждёт её и берёт результат из кеша, а не качает
/// ещё раз (и опоздавший сбой не затирает удачу соседа).
static LOADING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Каталог из памяти или из сети; сбой — в лог и `None`.
pub async fn load() -> Option<Arc<Catalog>> {
    let _single = LOADING.lock().await;
    if let Some((at, cached)) = CACHE.lock().unwrap().as_ref()
        && at.elapsed() < if cached.is_some() { FRESH } else { RETRY }
    {
        return cached.clone();
    }
    let cached = match download().await {
        Ok(catalog) => Some(Arc::new(catalog)),
        Err(exc) => {
            tracing::warn!(error = %exc, "OpenCode model catalog unavailable");
            None
        }
    };
    *CACHE.lock().unwrap() = Some((Instant::now(), cached.clone()));
    cached
}

async fn download() -> Result<Catalog, String> {
    let client = ctp_gateway::proxy::apply(reqwest::Client::builder())
        .user_agent(ctp_gateway::state::USER_AGENT)
        .timeout(TIMEOUT)
        .build()
        .map_err(|exc| exc.to_string())?;
    // reqwest собран без gzip (он общий со шлюзом) — распаковываем сами: 0,8 МБ вместо 11.
    let response = client
        .get(URL)
        .header("accept-encoding", "gzip")
        .send()
        .await
        .map_err(|exc| exc.to_string())?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status().as_u16()));
    }
    let gzip = response
        .headers()
        .get("content-encoding")
        .is_some_and(|value| value.as_bytes().eq_ignore_ascii_case(b"gzip"));
    if response.content_length().is_some_and(|len| len > MAX_BYTES) {
        return Err(tr!("ответ больше 64 МБ", "response is larger than 64 MB"));
    }
    let body = response.bytes().await.map_err(|exc| exc.to_string())?;
    tokio::task::spawn_blocking(move || parse(&body, gzip))
        .await
        .map_err(|exc| exc.to_string())?
}

fn parse(body: &[u8], gzip: bool) -> Result<Catalog, String> {
    let too_big = || tr!("каталог больше 64 МБ", "catalog is larger than 64 MB");
    let mut text = Vec::new();
    let json = if gzip {
        flate2::read::GzDecoder::new(body)
            .take(MAX_BYTES + 1)
            .read_to_end(&mut text)
            .map_err(|exc| exc.to_string())?;
        &text[..]
    } else {
        body
    };
    if json.len() as u64 > MAX_BYTES {
        return Err(too_big());
    }
    let catalog: Catalog = serde_json::from_slice(json).map_err(|exc| exc.to_string())?;
    // `{"error": …}` или другая схема разбираются «успешно» в пустоту — это сбой, не каталог.
    if !catalog
        .0
        .values()
        .any(|provider| provider.0.as_ref().is_some_and(|p| !p.models.is_empty()))
    {
        return Err(tr!(
            "в ответе нет провайдеров с моделями",
            "no providers with models in the response"
        ));
    }
    Ok(catalog)
}

/// `https://API.x.com/v1/` → `https://api.x.com`: у DeepSeek в каталоге адрес без `/v1`.
fn normalize(url: &str) -> String {
    let url = url.trim().trim_end_matches('/').to_ascii_lowercase();
    url.strip_suffix("/v1").unwrap_or(&url).to_string()
}

/// Подсказки к `ids` провайдера с адресом `base_url`. Кандидаты — провайдеры каталога,
/// у моделей которых тот же адрес API, иначе адрес под введённым (`…/inference/go` →
/// `…/inference/go/openai/v1`). Из них годятся только знающие хоть один из `ids`
/// (у QVAC в каталоге адрес Ollama `127.0.0.1:11434`); источник — кто знает больше,
/// недостающее добирается у остальных (Perplexity и Perplexity Agent на одном адресе).
pub fn hints(catalog: &Catalog, base_url: &str, ids: &[String]) -> Hints {
    let base = normalize(base_url);
    if base.is_empty() || ids.is_empty() {
        return Hints::default();
    }
    let nested = format!("{base}/");
    let serves = |provider: &CatProvider, exact: bool| {
        provider
            .models
            .values()
            .filter_map(|model| model.0.as_ref())
            .flat_map(|model| model.api.values())
            .filter_map(|api| api.0.as_ref()?.base_url.as_deref().map(normalize))
            .any(|url| {
                if exact {
                    url == base
                } else {
                    url.starts_with(&nested)
                }
            })
    };
    fn model<'a>(provider: &'a CatProvider, id: &str) -> Option<&'a CatModel> {
        provider.models.get(id)?.0.as_ref()
    }
    let overlap = |provider: &CatProvider| {
        ids.iter()
            .filter(|id| model(provider, id).is_some())
            .count()
    };
    let candidates = |exact: bool| {
        let mut found: Vec<(&String, &CatProvider, usize)> = catalog
            .0
            .iter()
            .filter_map(|(id, provider)| {
                let provider = provider.0.as_ref()?;
                serves(provider, exact).then(|| (id, provider, overlap(provider)))
            })
            .filter(|(_, _, overlap)| *overlap > 0)
            .collect();
        // Больше общих моделей — раньше; при равенстве по ID, чтобы не зависеть от HashMap.
        found.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(b.0)));
        found
    };
    let mut found = candidates(true);
    if found.is_empty() {
        found = candidates(false);
    }
    let Some((first_id, first, _)) = found.first() else {
        return Hints::default();
    };
    let mut models = HashMap::new();
    for (_, provider, _) in &found {
        for id in ids {
            if !models.contains_key(id)
                && let Some(model) = model(provider, id)
            {
                models.insert(id.clone(), hint(model));
            }
        }
    }
    Hints {
        source: Some(first.name.clone().unwrap_or_else(|| (*first_id).clone())),
        models,
    }
}

fn hint(model: &CatModel) -> Hint {
    let level = |value: &str| {
        let value = value.trim().to_ascii_lowercase();
        CLAUDE_LEVELS.iter().copied().find(|level| *level == value)
    };
    let reasoning = model
        .capabilities
        .as_ref()
        .and_then(|caps| caps.reasoning.as_ref())
        .filter(|reasoning| reasoning.supported);
    let mut efforts: Vec<&'static str> = reasoning
        .map(|r| {
            r.effort
                .iter()
                .filter_map(|e| level(e.0.as_deref()?))
                .collect()
        })
        .unwrap_or_default();
    efforts.sort_by_key(|e| CLAUDE_LEVELS.iter().position(|l| l == e));
    efforts.dedup();
    let default_effort = reasoning
        .and_then(|r| r.default.as_ref())
        .and_then(|d| d.value.as_deref())
        .and_then(level)
        .filter(|level| efforts.contains(level));
    Hint {
        name: model.name.clone().filter(|name| !name.trim().is_empty()),
        context: model.limit.as_ref().and_then(|limit| limit.context),
        output: model.limit.as_ref().and_then(|limit| limit.output),
        efforts,
        default_effort,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Catalog {
        serde_json::from_value(serde_json::json!({
            "opencode-go": {"name": "OpenCode Go", "models": {
                "deepseek-v4-pro": {
                    "name": "DeepSeek V4 Pro", "limit": {"context": 1000000, "output": 384000},
                    "capabilities": {"reasoning": {"supported": true, "effort": ["max", "high"]}},
                    "api": {"chat-completions": {"base_url": "https://opencode.ai/inference/go/openai/v1"}}
                },
                "qwen3.8-flash": {
                    "name": "Qwen3.8 Flash",
                    "capabilities": {"reasoning": {"supported": true, "effort": ["none", "low", "xhigh"],
                                                   "default": {"type": "effort", "value": "medium"}}},
                    "api": {"messages": {"base_url": "https://opencode.ai/inference/go/anthropic/v1"}}
                }
            }},
            "deepseek": {"name": "DeepSeek", "models": {
                "deepseek-v4-flash": {
                    "name": "DeepSeek V4 Flash", "limit": {"context": 1048576},
                    "capabilities": {"reasoning": {"supported": true, "effort": ["low", "high", "max"],
                                                   "default": {"value": "high"}}},
                    "api": {"chat-completions": {"base_url": "https://api.deepseek.com"}}
                }
            }},
            "odd": {"name": 5, "models": {
                "m": {"unknown": 1, "api": {"x": {}}},
                "toggle": {"name": "T", "capabilities": {"reasoning": {"supported": true,
                    "default": {"type": "toggle", "value": true}, "effort": ["high", 7]}},
                    "api": {"chat": {"base_url": "https://odd.example/v1"}, "bad": 3}},
                "junk": 42
            }},
            "broken": 17,
            "qvac": {"name": "QVAC", "models": {
                "qvac-1": {"api": {"chat": {"base_url": "http://127.0.0.1:11434/v1"}}}
            }},
            "perplexity": {"name": "Perplexity", "models": {
                "sonar": {"name": "Sonar", "api": {"chat": {"base_url": "https://api.perplexity.ai"}}},
                "a": {"name": "A (plain)", "api": {"chat": {"base_url": "https://api.perplexity.ai"}}}
            }},
            "perplexity-agent": {"name": "Perplexity Agent", "models": {
                "a": {"name": "A", "api": {"chat": {"base_url": "https://api.perplexity.ai"}}},
                "b": {"name": "B", "api": {"chat": {"base_url": "https://api.perplexity.ai"}}},
                "c": {"name": "C", "api": {"chat": {"base_url": "https://api.perplexity.ai"}}}
            }}
        }))
        .unwrap()
    }

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|id| id.to_string()).collect()
    }

    #[test]
    fn finds_provider_by_api_address() {
        let catalog = sample();
        let go_ids = ids(&["deepseek-v4-pro", "qwen3.8-flash", "unknown-model"]);
        let go = hints(
            &catalog,
            "https://opencode.ai/inference/go/openai/v1/",
            &go_ids,
        );
        assert_eq!(go.source.as_deref(), Some("OpenCode Go"));
        let pro = &go.models["deepseek-v4-pro"];
        assert_eq!(pro.name.as_deref(), Some("DeepSeek V4 Pro"));
        assert_eq!(pro.context, Some(1_000_000));
        assert_eq!(pro.output, Some(384_000));
        assert_eq!(pro.efforts, vec!["high", "max"]);
        // Подсказки — только к полученным ID.
        assert_eq!(go.models.len(), 2);
        // Anthropic-адрес того же провайдера и адрес-«родитель» тоже находят Go.
        let anthropic = hints(
            &catalog,
            "https://opencode.ai/inference/go/anthropic",
            &go_ids,
        );
        assert_eq!(anthropic.source.as_deref(), Some("OpenCode Go"));
        let parent = hints(&catalog, "https://opencode.ai/inference/go/v1", &go_ids);
        assert_eq!(parent.source.as_deref(), Some("OpenCode Go"));
        // `/v1` в пресете DeepSeek и без него в каталоге.
        let ds = hints(
            &catalog,
            "https://API.deepseek.com/v1",
            &ids(&["deepseek-v4-flash"]),
        );
        assert_eq!(ds.models["deepseek-v4-flash"].default_effort, Some("high"));
        assert!(hints(&catalog, "", &go_ids).source.is_none());
        assert!(
            hints(&catalog, "https://api.deepseek.com", &[])
                .source
                .is_none()
        );
    }

    #[test]
    fn same_address_needs_shared_models_and_merges_providers() {
        let catalog = sample();
        // У QVAC адрес Ollama, но моделей Ollama он не знает — не он.
        let ollama = hints(&catalog, "http://127.0.0.1:11434/v1", &ids(&["qwen3:8b"]));
        assert!(ollama.source.is_none());
        // Perplexity Agent знает больше полученных ID, sonar добирается у Perplexity.
        let pplx = hints(
            &catalog,
            "https://api.perplexity.ai",
            &ids(&["a", "b", "c", "sonar"]),
        );
        assert_eq!(pplx.source.as_deref(), Some("Perplexity Agent"));
        assert_eq!(pplx.models["sonar"].name.as_deref(), Some("Sonar"));
        assert_eq!(
            pplx.models["a"].name.as_deref(),
            Some("A"),
            "общая — у главного"
        );
        assert_eq!(pplx.models.len(), 4);
    }

    #[test]
    fn efforts_keep_only_claude_levels_and_known_default() {
        let found = hints(
            &sample(),
            "https://opencode.ai/inference/go/anthropic",
            &ids(&["qwen3.8-flash"]),
        );
        let qwen = &found.models["qwen3.8-flash"];
        assert_eq!(qwen.efforts, vec!["low", "xhigh"]);
        // medium нет среди уровней модели — по умолчанию не подставляем.
        assert_eq!(qwen.default_effort, None);
        assert_eq!(qwen.context, None);
    }

    #[test]
    fn odd_entries_are_skipped_not_fatal() {
        let odd = hints(
            &sample(),
            "https://odd.example",
            &ids(&["toggle", "junk", "m"]),
        );
        // Имя провайдера не строка — берётся ID; булев default и число в effort — мимо.
        assert_eq!(odd.source.as_deref(), Some("odd"));
        let toggle = &odd.models["toggle"];
        assert_eq!(toggle.efforts, vec!["high"]);
        assert_eq!(toggle.default_effort, None);
        assert!(!odd.models.contains_key("junk"));
    }

    #[test]
    fn gzip_body_is_unpacked_and_empty_answers_rejected() {
        use std::io::Write;
        let json = br#"{"p": {"name": "P", "models": {"m": {}}}}"#;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(json).unwrap();
        let packed = encoder.finish().unwrap();
        let catalog = parse(&packed, true).unwrap();
        assert_eq!(
            catalog.0["p"].0.as_ref().unwrap().name.as_deref(),
            Some("P")
        );
        assert!(parse(json, false).is_ok());
        assert!(parse(b"not gzip", true).is_err());
        // Ошибка сервера в виде JSON или чужая схема — не каталог.
        assert!(parse(br#"{"error": "busy"}"#, false).is_err());
        assert!(parse(br#"{"p": {"models": {}}}"#, false).is_err());
    }

    /// Живой каталог: `cargo test -p ctp-app catalog::tests::live -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn live() {
        let catalog = load().await.expect("каталог");
        let found = hints(
            &catalog,
            "https://opencode.ai/inference/go/openai/v1",
            &ids(&["deepseek-v4-pro", "glm-5.2", "gpt-5.6-luna"]),
        );
        assert_eq!(found.source.as_deref(), Some("OpenCode Go"));
        assert_eq!(found.models.len(), 3);
        assert_eq!(found.models["deepseek-v4-pro"].efforts, vec!["high", "max"]);
    }
}
