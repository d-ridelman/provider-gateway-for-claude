use ctp_core::alias::strip_1m;
use ctp_core::config::{ProviderConfig, ProvidersFile};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderKind {
    OpenAi,
    Anthropic,
}

#[derive(Clone, Debug)]
pub struct Provider {
    pub id: String,
    pub name: String,
    pub kind: ProviderKind,
    pub base_url: String,
    pub api_key: String,
    pub headers: BTreeMap<String, String>,
    pub chat_completions_url: Option<String>,
    pub max_tokens_field: Option<String>,
    pub extra_body: serde_json::Map<String, Value>,
    pub include_usage: bool,
    /// Потолок уровня рассуждения; `None` — `reasoning_effort` провайдеру не шлём.
    pub max_effort: Option<&'static str>,
    pub default_effort: Option<&'static str>,
    /// Заголовок для ID разговора (см. `ProviderConfig::session_header`).
    pub session_header: Option<String>,
}

/// Заголовок ID разговора: из конфига, а для OpenCode (`opencode.ai`) — `x-opencode-session`,
/// без него OpenCode Go отвечает `MissingSessionID`.
fn session_header_for(cfg: &ProviderConfig) -> Option<String> {
    if let Some(name) = cfg.session_header.as_deref().map(str::trim) {
        if name.is_empty() || name.eq_ignore_ascii_case("off") {
            return None;
        }
        if axum::http::HeaderName::from_bytes(name.as_bytes()).is_err() {
            tracing::warn!(provider = %cfg.id, name, "invalid session_header skipped");
            return None;
        }
        return Some(name.to_ascii_lowercase());
    }
    let host = cfg
        .base_url
        .split("://")
        .nth(1)
        .and_then(|rest| rest.split(['/', ':']).next())
        .unwrap_or("")
        .to_ascii_lowercase();
    (host == "opencode.ai" || host.ends_with(".opencode.ai"))
        .then(|| "x-opencode-session".to_string())
}

/// ID разговора Claude Code: заголовок `X-Claude-Code-Session-Id`, иначе `metadata.user_id`
/// (JSON с `session_id` у новых версий, `user_…_session_<id>` у старых).
pub fn session_id(headers: &axum::http::HeaderMap, body: &Value) -> Option<String> {
    let header = headers
        .get("x-claude-code-session-id")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if let Some(id) = header {
        return Some(id.to_string());
    }
    let user = body.pointer("/metadata/user_id")?.as_str()?;
    let id = match serde_json::from_str::<Value>(user) {
        Ok(parsed) => parsed.get("session_id")?.as_str()?.to_string(),
        Err(_) => user.rsplit_once("_session_")?.1.to_string(),
    };
    (!id.is_empty()).then_some(id)
}

impl Provider {
    /// Заголовок ID разговора и значение, если провайдер его ждёт. Нет ID от Claude
    /// (проверка, список моделей) или он не годится в заголовок — свежий: без кеша провайдера
    /// лучше, чем отказ. Тот же заголовок в `headers` провайдера главнее — тогда ничего.
    pub fn session_pair(
        &self,
        session: Option<&str>,
    ) -> Option<(axum::http::HeaderName, axum::http::HeaderValue)> {
        let name =
            axum::http::HeaderName::from_bytes(self.session_header.as_deref()?.as_bytes()).ok()?;
        if self
            .headers
            .keys()
            .any(|key| key.eq_ignore_ascii_case(name.as_str()))
        {
            return None;
        }
        let value = session
            // http пропускает байты ≥ 0x80, провайдеры — нет: только печатный ASCII.
            .filter(|id| id.bytes().all(|b| b.is_ascii_graphic()))
            .and_then(|id| axum::http::HeaderValue::from_str(id).ok())
            .or_else(|| {
                let fresh = format!("ses_{}", uuid::Uuid::new_v4().simple());
                axum::http::HeaderValue::from_str(&fresh).ok()
            })?;
        Some((name, value))
    }
}

pub const USER_AGENT: &str = concat!("provider-gateway-for-claude/", env!("CARGO_PKG_VERSION"));

/// Уровни рассуждения Claude по возрастанию (как в Claude Desktop).
pub const EFFORT_LEVELS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

fn effort_level(value: Option<&str>, provider: &str, field: &str) -> Option<&'static str> {
    let value = value?.trim().to_ascii_lowercase();
    if value.is_empty() || value == "off" {
        return None;
    }
    let level = EFFORT_LEVELS.iter().copied().find(|l| *l == value);
    if level.is_none() {
        tracing::warn!(provider, field, value, "unknown reasoning level skipped");
    }
    level
}

impl Provider {
    /// `reasoning_effort` для провайдера: уровень из запроса Claude (`output_config.effort`)
    /// или уровень по умолчанию, не выше потолка. Без потолка — ничего.
    pub fn reasoning_effort(&self, body: &Value) -> Option<&'static str> {
        let max = self.max_effort?;
        // Значение из запроса без предупреждения: его пишет клиент, а не человек в конфиге.
        let requested = body
            .pointer("/output_config/effort")
            .and_then(Value::as_str)
            .map(|e| e.trim().to_ascii_lowercase())
            .and_then(|e| EFFORT_LEVELS.iter().copied().find(|l| *l == e))
            .or(self.default_effort)?;
        let rank = |level: &str| EFFORT_LEVELS.iter().position(|l| *l == level).unwrap_or(0);
        Some(if rank(requested) > rank(max) {
            max
        } else {
            requested
        })
    }

    /// URL OpenAI Chat Completions. `base_url` может быть и `https://host`,
    /// и `https://host/v1`, и `https://host/v1beta/openai` (Gemini).
    pub fn chat_url(&self) -> String {
        if let Some(url) = &self.chat_completions_url {
            return url.clone();
        }
        format!("{}/chat/completions", self.openai_root())
    }

    /// URL списка моделей: OpenAI `…/models` рядом с `chat/completions`,
    /// Anthropic `…/v1/models`.
    pub fn models_url(&self) -> String {
        if self.kind == ProviderKind::Anthropic {
            return self.anthropic_url("/models", None);
        }
        match self
            .chat_completions_url
            .as_deref()
            .and_then(|url| url.strip_suffix("/chat/completions"))
        {
            Some(root) => format!("{root}/models"),
            None => format!("{}/models", self.openai_root()),
        }
    }

    fn openai_root(&self) -> String {
        let base = self.base_url.trim_end_matches('/');
        if base.ends_with("/v1")
            || base.ends_with("/openai")
            || base.ends_with("/v1beta/openai")
            || base.ends_with("/api/v1")
        {
            base.to_string()
        } else {
            format!("{base}/v1")
        }
    }

    /// URL Anthropic Messages. `https://host` → `https://host/v1/messages`,
    /// `https://host/v1` → `https://host/v1/messages`.
    pub fn anthropic_url(&self, path: &str, query: Option<&str>) -> String {
        let base = self.base_url.trim_end_matches('/');
        let base = base.strip_suffix("/v1").unwrap_or(base);
        let mut url = format!("{base}/v1{path}");
        if let Some(query) = query {
            url.push('?');
            url.push_str(query);
        }
        url
    }

    /// Имя поля лимита ответа. Свежие модели OpenAI хотят `max_completion_tokens`,
    /// остальные — `max_tokens`.
    pub fn max_tokens_key(&self, model: &str) -> Option<String> {
        match self.max_tokens_field.as_deref() {
            Some("none") => None,
            Some("max_completion_tokens") => Some("max_completion_tokens".into()),
            Some("max_tokens") => Some("max_tokens".into()),
            Some(_) => Some("max_tokens".into()),
            None if is_new_openai_model(model) => Some("max_completion_tokens".into()),
            None => Some("max_tokens".into()),
        }
    }
}

fn is_new_openai_model(model: &str) -> bool {
    let model = model.to_ascii_lowercase();
    model.contains("gpt-5")
        || model.starts_with("o1")
        || model.starts_with("o3")
        || model.starts_with("o4")
}

#[derive(Clone, Debug)]
pub struct Route {
    pub provider: usize,
    pub model: String,
    pub alias: String,
}

#[derive(Clone, Debug)]
pub struct ModelRouter {
    pub providers: Vec<Provider>,
    pub routes: Vec<(String, Route)>,
    route_index: HashMap<String, usize>,
    by_model: HashMap<String, Route>,
    pub default: Option<Route>,
}

impl ModelRouter {
    pub fn new(file: ProvidersFile) -> anyhow::Result<Self> {
        if file.providers.is_empty() {
            anyhow::bail!("no providers configured");
        }

        let mut providers = Vec::with_capacity(file.providers.len());
        let mut routes: Vec<(String, Route)> = Vec::new();
        let mut route_index: HashMap<String, usize> = HashMap::new();
        let mut by_model: HashMap<String, Route> = HashMap::new();

        for cfg in file.providers {
            let provider = Provider::from_config(&cfg)?;
            let provider_index = providers.len();
            providers.push(provider);

            for (alias, model) in &cfg.models {
                let route = Route {
                    provider: provider_index,
                    model: model.clone(),
                    alias: alias.clone(),
                };
                if let Some(&index) = route_index.get(alias) {
                    // Как у Python: повторный псевдоним заменяет маршрут, но не порядок.
                    routes[index].1 = route.clone();
                } else {
                    route_index.insert(alias.clone(), routes.len());
                    routes.push((alias.clone(), route.clone()));
                }
                by_model.entry(model.clone()).or_insert(route);
            }
        }

        let default = file
            .default
            .as_deref()
            .and_then(|name| {
                let name = strip_1m(name);
                route_index
                    .get(name)
                    .map(|&index| routes[index].1.clone())
                    .or_else(|| by_model.get(name).cloned())
            })
            .or_else(|| routes.first().map(|(_, route)| route.clone()));

        for provider in &providers {
            if provider.base_url.is_empty() {
                anyhow::bail!("provider {:?}: empty base_url", provider.id);
            }
        }

        Ok(Self {
            providers,
            routes,
            route_index,
            by_model,
            default,
        })
    }

    /// Псевдоним, настоящий ID модели или модель по умолчанию.
    pub fn resolve(&self, requested: &str) -> Option<Route> {
        let name = strip_1m(requested);
        if let Some(&index) = self.route_index.get(name) {
            return Some(self.routes[index].1.clone());
        }
        if let Some(route) = self.by_model.get(name) {
            return Some(route.clone());
        }
        self.default.clone()
    }
}

impl Provider {
    pub fn from_config(cfg: &ProviderConfig) -> anyhow::Result<Self> {
        Ok(Self {
            id: cfg.id.clone(),
            name: cfg.name().to_string(),
            kind: if cfg.is_anthropic() {
                ProviderKind::Anthropic
            } else {
                ProviderKind::OpenAi
            },
            base_url: cfg.base_url.clone(),
            api_key: cfg.api_key().to_string(),
            headers: cfg.headers.clone(),
            chat_completions_url: cfg.chat_completions_url.clone(),
            max_tokens_field: cfg.max_tokens_field.clone(),
            extra_body: cfg.extra_body.clone().unwrap_or_default(),
            include_usage: cfg.include_usage.unwrap_or(false),
            max_effort: effort_level(cfg.max_effort.as_deref(), &cfg.id, "max_effort"),
            default_effort: effort_level(cfg.default_effort.as_deref(), &cfg.id, "default_effort"),
            session_header: session_header_for(cfg),
        })
    }
}

#[derive(Clone)]
pub struct AppState {
    pub router: ModelRouter,
    pub token: String,
    pub open_mode: bool,
    pub oauth_scope: String,
    pub debug_prompts: bool,
    pub clients: Clients,
}

#[derive(Clone)]
pub struct Clients {
    pub json: reqwest::Client,
    pub stream: reqwest::Client,
}

impl Clients {
    pub fn new() -> anyhow::Result<Self> {
        // Свой User-Agent, а не reqwest: OpenCode Go и др. просят клиента представляться.
        let json = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(Duration::from_secs(60))
            .timeout(Duration::from_secs(1800))
            .build()?;
        let stream = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(Duration::from_secs(60))
            .read_timeout(Duration::from_secs(1800))
            .build()?;
        Ok(Self { json, stream })
    }
}

impl AppState {
    pub fn new(
        router: ModelRouter,
        token: String,
        open_mode: bool,
        oauth_scope: String,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            router,
            token,
            open_mode,
            oauth_scope,
            debug_prompts: false,
            clients: Clients::new()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ctp_core::config::ProviderConfig;

    fn provider(id: &str, kind: &str, models: Vec<(&str, &str)>) -> ProviderConfig {
        ProviderConfig {
            id: id.into(),
            name: Some(id.into()),
            kind: Some(kind.into()),
            base_url: format!("https://{id}.example/v1"),
            api_key: Some("sk-test".into()),
            models: models
                .into_iter()
                .map(|(a, m)| (a.to_string(), m.to_string()))
                .collect(),
            headers: BTreeMap::new(),
            chat_completions_url: None,
            max_tokens_field: None,
            extra_body: None,
            include_usage: None,
            max_effort: None,
            default_effort: None,
            session_header: None,
        }
    }

    #[test]
    fn session_header_is_auto_for_opencode_and_configurable() {
        let header = |base: &str, set: Option<&str>| {
            let mut cfg = provider("p", "openai", vec![("a", "m")]);
            cfg.base_url = base.into();
            cfg.session_header = set.map(str::to_string);
            Provider::from_config(&cfg).unwrap().session_header
        };
        let go = "https://opencode.ai/inference/go/openai/v1";
        assert_eq!(header(go, None).as_deref(), Some("x-opencode-session"));
        assert_eq!(header(go, Some("OFF")), None);
        assert_eq!(header("https://notopencode.ai/v1", None), None);
        assert_eq!(
            header("https://api.example/v1", Some("X-Conv")).as_deref(),
            Some("x-conv")
        );

        let none = axum::http::HeaderMap::new();
        let legacy = serde_json::json!({"metadata": {"user_id": "user_ab_account__session_s-1"}});
        assert_eq!(session_id(&none, &legacy).as_deref(), Some("s-1"));
        assert_eq!(session_id(&none, &serde_json::json!({})), None);
        // Без ID от Claude — свежий ID, но заголовок всё равно уходит.
        let p = Provider::from_config(&{
            let mut cfg = provider("p", "openai", vec![("a", "m")]);
            cfg.base_url = go.into();
            cfg
        })
        .unwrap();
        let value = |id: Option<&str>| p.session_pair(id).unwrap().1.to_str().unwrap().to_string();
        assert!(value(None).starts_with("ses_"));
        assert_eq!(value(Some("s-2")), "s-2");
        // ID, который не годится в заголовок, заменяется свежим, а не теряется.
        assert!(value(Some("сессия")).starts_with("ses_"));
        assert!(value(Some("a\nb")).starts_with("ses_"));
        assert_eq!(header("https://api.example/v1", Some("X Conv")), None);
        // Свой заголовок в headers провайдера главнее автоматического.
        let mut own = p.clone();
        own.headers
            .insert("X-OpenCode-Session".into(), "fixed".into());
        assert!(own.session_pair(Some("s-2")).is_none());
    }

    #[test]
    fn reasoning_effort_is_opt_in_clamped_and_defaulted() {
        let with = |max: Option<&str>, default: Option<&str>| {
            let mut cfg = provider("p", "openai", vec![("a", "m")]);
            cfg.max_effort = max.map(str::to_string);
            cfg.default_effort = default.map(str::to_string);
            Provider::from_config(&cfg).unwrap()
        };
        let asked = |effort: &str| serde_json::json!({"output_config": {"effort": effort}});
        let none = serde_json::json!({});

        // Выключено: ничего не шлём, даже если Claude попросил.
        assert_eq!(
            with(None, Some("high")).reasoning_effort(&asked("max")),
            None
        );
        assert_eq!(
            with(Some("off"), None).reasoning_effort(&asked("max")),
            None
        );
        // Передаём как есть и обрезаем потолком.
        let up_to_high = with(Some("high"), None);
        assert_eq!(up_to_high.reasoning_effort(&asked("low")), Some("low"));
        assert_eq!(up_to_high.reasoning_effort(&asked("max")), Some("high"));
        assert_eq!(up_to_high.reasoning_effort(&asked("XHIGH")), Some("high"));
        // Без уровня в запросе — по умолчанию (тоже не выше потолка), иначе ничего.
        assert_eq!(up_to_high.reasoning_effort(&none), None);
        assert_eq!(
            with(Some("max"), Some("medium")).reasoning_effort(&none),
            Some("medium")
        );
        assert_eq!(
            with(Some("low"), Some("max")).reasoning_effort(&none),
            Some("low")
        );
        // Неизвестное значение в запросе — как отсутствующее.
        assert_eq!(
            with(Some("max"), Some("high")).reasoning_effort(&asked("turbo")),
            Some("high")
        );
        // Неизвестный потолок в конфиге — функция выключена.
        assert_eq!(
            with(Some("ultra"), None).reasoning_effort(&asked("low")),
            None
        );
    }

    #[test]
    fn routes_by_alias_model_and_default() {
        let router = ModelRouter::new(ProvidersFile {
            default: Some("claude-openai-80673633".into()),
            providers: vec![
                provider(
                    "openai",
                    "openai",
                    vec![("claude-openai-80673633", "gpt-5.2")],
                ),
                provider(
                    "anthropic",
                    "anthropic",
                    vec![("claude-anthropic-18183586", "claude-sonnet-4-5")],
                ),
            ],
        })
        .unwrap();

        let alias = router.resolve("claude-openai-80673633[1m]").unwrap();
        assert_eq!(alias.model, "gpt-5.2");
        let real = router.resolve("claude-sonnet-4-5").unwrap();
        assert_eq!(real.provider, 1);
        let unknown = router.resolve("claude-haiku-4-5").unwrap();
        assert_eq!(unknown.model, "gpt-5.2");
    }

    #[test]
    fn urls_are_stable() {
        let p = Provider {
            id: "x".into(),
            name: "X".into(),
            kind: ProviderKind::OpenAi,
            base_url: "https://host/v1".into(),
            api_key: String::new(),
            headers: BTreeMap::new(),
            chat_completions_url: None,
            max_tokens_field: None,
            extra_body: Default::default(),
            include_usage: false,
            max_effort: None,
            default_effort: None,
            session_header: None,
        };
        assert_eq!(p.chat_url(), "https://host/v1/chat/completions");
        assert_eq!(
            p.anthropic_url("/messages", Some("beta=true")),
            "https://host/v1/messages?beta=true"
        );

        let gemini = Provider {
            base_url: "https://generativelanguage.googleapis.com/v1beta/openai".into(),
            ..p.clone()
        };
        assert_eq!(
            gemini.chat_url(),
            "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions"
        );

        let bare = Provider {
            base_url: "https://api.deepseek.com".into(),
            ..p
        };
        assert_eq!(
            bare.chat_url(),
            "https://api.deepseek.com/v1/chat/completions"
        );
        assert_eq!(bare.models_url(), "https://api.deepseek.com/v1/models");
        assert_eq!(
            gemini.models_url(),
            "https://generativelanguage.googleapis.com/v1beta/openai/models"
        );

        let azure = Provider {
            chat_completions_url: Some("https://az/openai/deployments/x/chat/completions".into()),
            ..bare.clone()
        };
        assert_eq!(azure.models_url(), "https://az/openai/deployments/x/models");

        let anthropic = Provider {
            kind: ProviderKind::Anthropic,
            base_url: "https://api.anthropic.com".into(),
            ..bare
        };
        assert_eq!(
            anthropic.models_url(),
            "https://api.anthropic.com/v1/models"
        );
    }
}
