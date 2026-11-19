use crate::model::{ModelInfo, UiStatus};
use crate::state::AppShared;
use crate::{store, tr};
use ctp_gateway::state::{AppState, ModelRouter};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::oneshot;

pub struct GatewayRuntime {
    pub port: u16,
    pub token: String,
    pub models: Vec<ModelInfo>,
    shutdown: Option<oneshot::Sender<()>>,
    alive: Arc<AtomicBool>,
}

/// Сбрасывает флаг и при обычном завершении задачи сервера, и при панике.
struct AliveGuard(Arc<AtomicBool>);

impl Drop for AliveGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

pub async fn start(shared: &AppShared) -> Result<UiStatus, String> {
    // Номер берём из самой остановки: два параллельных запуска получат разные номера.
    let epoch = stop(shared);
    // Конфиг читаем после остановки: сохранение, случившееся позже, снова поднимет epoch.
    let config = { shared.config.lock().unwrap().clone() };

    let file = store::build_file(&config)?;
    let router = ModelRouter::new(file)
        .map_err(|exc| tr!("Конфиг провайдеров: {exc}", "Provider config: {exc}"))?;

    let rows: HashMap<String, crate::model::AliasRow> = config
        .providers
        .iter()
        .flat_map(store::alias_rows)
        .map(|row| (row.alias.clone(), row))
        .collect();
    let models: Vec<ModelInfo> = router
        .routes
        .iter()
        .map(|(alias, route)| {
            let provider = &router.providers[route.provider];
            ModelInfo {
                alias: alias.clone(),
                model: route.model.clone(),
                provider: provider.name.clone(),
                supports_1m: rows.get(alias).is_some_and(|row| row.supports_1m),
                max_effort: provider.max_effort.map(str::to_string),
                openai: provider.kind == ctp_gateway::state::ProviderKind::OpenAi,
                label: rows.get(alias).and_then(|row| row.label.clone()),
            }
        })
        .collect();

    let token = config.token.clone();
    if token.is_empty() {
        return Err(tr!("Пустой локальный токен", "Local token is empty"));
    }
    let gateway_state = AppState::new(
        router,
        token.clone(),
        false,
        "user:inference user:file_upload user:profile user:sessions:claude_code".into(),
    )
    .map_err(|exc| tr!("Шлюз: {exc}", "Gateway: {exc}"))?;

    let Some(listener) = bind(shared, config.port, epoch).await? else {
        tracing::info!("gateway start superseded by a later command");
        return Ok(status(shared));
    };
    let port = listener.local_addr().map_err(|exc| exc.to_string())?.port();

    let (shutdown, wait) = oneshot::channel();
    let alive = Arc::new(AtomicBool::new(true));
    let guard = AliveGuard(alive.clone());
    tauri::async_runtime::spawn(async move {
        let _guard = guard;
        let server =
            axum::serve(listener, ctp_gateway::app(gateway_state)).with_graceful_shutdown(async {
                let _ = wait.await;
            });
        if let Err(exc) = server.await {
            tracing::error!(error = %exc, "gateway stopped with an error");
        }
    });

    {
        let mut slot = shared.runtime.lock().unwrap();
        if shared.epoch.load(Ordering::SeqCst) != epoch {
            // Пока шёл bind, шлюз остановили или запустили заново — этот сервер лишний.
            let _ = shutdown.send(());
            drop(slot);
            tracing::info!(port, "gateway start superseded by a later command");
            return Ok(status(shared));
        }
        *slot = Some(GatewayRuntime {
            port,
            token,
            models,
            shutdown: Some(shutdown),
            alive,
        });
    }
    tracing::info!(port, "gateway started");
    Ok(status(shared))
}

/// Только что остановленный шлюз может ещё держать порт — пробуем до ~2 с.
/// `None` — пока ждали порт, этот запуск перебили остановкой или другим запуском.
async fn bind(
    shared: &AppShared,
    port: u16,
    epoch: u64,
) -> Result<Option<tokio::net::TcpListener>, String> {
    let mut attempt = 0;
    loop {
        if shared.epoch.load(Ordering::SeqCst) != epoch {
            return Ok(None);
        }
        match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
            Ok(listener) => return Ok(Some(listener)),
            Err(exc) if exc.kind() == std::io::ErrorKind::AddrInUse && attempt < 20 => {
                attempt += 1;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(exc) if exc.kind() == std::io::ErrorKind::AddrInUse => {
                return Err(tr!(
                    "Порт {port} занят — возможно, уже открыто другое окно шлюза",
                    "Port {port} is busy; another gateway window may already be open"
                ));
            }
            Err(exc) => {
                return Err(tr!(
                    "Не удалось открыть порт {port}: {exc}",
                    "Could not open port {port}: {exc}"
                ));
            }
        }
    }
}

/// Останавливает шлюз и возвращает новый номер эпохи.
pub fn stop(shared: &AppShared) -> u64 {
    let epoch = shared.epoch.fetch_add(1, Ordering::SeqCst) + 1;
    if let Some(mut runtime) = shared.runtime.lock().unwrap().take() {
        if let Some(shutdown) = runtime.shutdown.take() {
            let _ = shutdown.send(());
        }
    }
    epoch
}

pub fn status(shared: &AppShared) -> UiStatus {
    let config = { shared.config.lock().unwrap().clone() };
    let claude = shared.claude.lock().unwrap().clone();
    let update = shared.update.lock().unwrap().clone();
    let mut runtime = shared.runtime.lock().unwrap();
    if runtime
        .as_ref()
        .is_some_and(|rt| !rt.alive.load(Ordering::SeqCst))
    {
        *runtime = None;
    }
    match runtime.as_ref() {
        Some(runtime) => UiStatus {
            running: true,
            port: runtime.port,
            token: runtime.token.clone(),
            models: runtime.models.clone(),
            log_path: shared.config_dir.join("gateway.log").display().to_string(),
            claude,
            update,
        },
        None => UiStatus {
            running: false,
            port: config.port,
            token: config.token,
            models: Vec::new(),
            log_path: shared.config_dir.join("gateway.log").display().to_string(),
            claude,
            update,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Config, ProviderRow};
    use serde_json::{Value, json};
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const SSE: &str = concat!(
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"streamed\"},",
        "\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n"
    );

    fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    async fn ask(port: u16, token: &str, stream: bool) -> (u16, String) {
        let response = reqwest::Client::new()
            .post(format!("http://127.0.0.1:{port}/v1/messages"))
            .header("x-api-key", token)
            .json(&json!({
                "model": "unknown-goes-to-default",
                "max_tokens": 64,
                "stream": stream,
                "messages": [{"role": "user", "content": "hi"}]
            }))
            .send()
            .await
            .unwrap();
        (response.status().as_u16(), response.text().await.unwrap())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn starts_serves_stops_and_restarts_on_same_port() {
        tauri::async_runtime::set(tokio::runtime::Handle::current());
        let upstream = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(body_string_contains("\"stream\":true"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(SSE, "text/event-stream"))
            .mount(&upstream)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "choices": [{"index": 0, "message": {"role": "assistant", "content": "pong"},
                             "finish_reason": "stop"}]
            })))
            .mount(&upstream)
            .await;

        let dir = std::env::temp_dir()
            .join("agents")
            .join(format!("ctp-app-rt-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        // Уникальный ID: ключа в Credential Manager для него нет, тест только читает.
        let id = format!("t{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
        let config = Config {
            port: free_port(),
            token: "local-test-token".into(),
            providers: vec![ProviderRow {
                id,
                name: "Mock".into(),
                kind: "openai".into(),
                base_url: format!("{}/v1", upstream.uri()),
                models: vec!["gpt-mock".into()],
                ..ProviderRow::default()
            }],
            ..Config::default()
        };
        let port = config.port;
        let shared = AppShared::new(dir.clone(), config);

        let status_now = start(&shared).await.unwrap();
        assert!(status_now.running);
        assert_eq!(status_now.models.len(), 1);

        let (code, body) = ask(port, "local-test-token", false).await;
        assert_eq!(code, 200, "{body}");
        let body: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(body["content"][0]["text"], "pong");

        let (code, sse) = ask(port, "local-test-token", true).await;
        assert_eq!(code, 200);
        assert!(sse.contains("event: message_start"), "{sse}");
        assert!(sse.contains("streamed"), "{sse}");
        assert!(sse.contains("event: message_stop"), "{sse}");

        assert_eq!(ask(port, "wrong", false).await.0, 401);

        // Перезапуск сразу на тот же порт — как после сохранения провайдера.
        assert!(start(&shared).await.unwrap().running);
        assert_eq!(ask(port, "local-test-token", false).await.0, 200);

        // Автозапуск и ручной «Запустить» одновременно: оба без ошибки, шлюз один и живой.
        let (first, second) = tokio::join!(start(&shared), start(&shared));
        assert!(first.is_ok() && second.is_ok(), "{first:?} {second:?}");
        assert!(status(&shared).running);
        assert_eq!(ask(port, "local-test-token", false).await.0, 200);

        stop(&shared);
        assert!(!status(&shared).running);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
