pub mod auth;
pub mod error;
pub mod handlers;
pub mod openai;
pub mod passthrough;
pub mod probe;
pub mod sse;
pub mod state;
pub mod util;
pub mod websearch;
pub mod websearch_tool;

use crate::state::{AppState, ModelRouter};
use crate::websearch::search;
use anyhow::{Context, bail};
use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{any, get, post};
use ctp_core::config::ProvidersFile;
use std::path::Path;

pub fn app(state: AppState) -> Router {
    Router::new()
        .route("/health", get(handlers::health))
        .route("/v1/models", get(handlers::models))
        .route("/v1/messages", post(handlers::messages))
        .route("/v1/messages/count_tokens", post(handlers::count_tokens))
        .route("/v1/oauth/{org}/authorize", post(handlers::oauth_authorize))
        .route("/v1/oauth/token", post(handlers::oauth_token))
        .route("/search", post(search))
        .route("/api/{*tail}", any(handlers::api_stub))
        .fallback(handlers::not_found)
        .layer(DefaultBodyLimit::max(256 * 1024 * 1024))
        .with_state(state)
}

pub fn state_from_env() -> anyhow::Result<AppState> {
    state_from_env_with(None)
}

pub fn state_from_env_with(providers_file: Option<&Path>) -> anyhow::Result<AppState> {
    let token = std::env::var("GATEWAY_TOKEN").unwrap_or_default();
    let open_mode = std::env::var("GATEWAY_OPEN").ok().as_deref() == Some("1");
    let oauth_scope = std::env::var("OAUTH_SCOPE").unwrap_or_else(|_| {
        "user:inference user:file_upload user:profile user:sessions:claude_code".to_string()
    });

    let file = load_providers(providers_file)?;

    if token.is_empty() && !open_mode {
        bail!("GATEWAY_TOKEN is required");
    }

    let router = ModelRouter::new(file)?;
    if router.routes.is_empty() {
        tracing::warn!("no models configured: /v1/models will be empty");
    }
    AppState::new(router, token, open_mode, oauth_scope)
}

fn load_providers(explicit: Option<&Path>) -> anyhow::Result<ProvidersFile> {
    if let Some(path) = explicit {
        return read_providers(path);
    }
    if let Ok(path) = std::env::var("PROVIDERS_FILE") {
        let path = path.trim();
        if !path.is_empty() {
            return read_providers(Path::new(path));
        }
    }
    if let Some(raw) = std::env::var("PROVIDERS_JSON")
        .ok()
        .filter(|raw| !raw.trim().is_empty())
    {
        return serde_json::from_str(&raw).context("PROVIDERS_JSON");
    }
    bail!("providers are not configured: pass --providers-file, PROVIDERS_FILE or PROVIDERS_JSON")
}

fn read_providers(path: &Path) -> anyhow::Result<ProvidersFile> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("providers file: {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("providers file: {}", path.display()))
}
