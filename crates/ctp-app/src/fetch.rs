//! Внутренний адрес для шлюза: загрузить одну страницу через встроенный браузер.
//!
//! Зачем: поисковики отвечают проверкой на бота (DuckDuckGo — `202` с капчей, Brave —
//! `429`), и тихий HTTP её не проходит. WebView2 проходит её сам, как живой посетитель,
//! а если она просит галочку — окно показывается, и человек её нажимает. Шлюз ходит
//! сюда `POST {"url"}`, получает HTML страницы и разбирает его своим парсером.
//!
//! Слушает только петля на случайном порту, а адрес (с секретом в пути) шлюз узнаёт из
//! окружения (`CTP_FETCH_URL`) при каждом поиске. Чужие процессы и страницы в браузере
//! секрета не знают, а запросы со страниц (с `Origin`) отклоняются в любом случае.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{Value, json};
use std::time::Duration;

/// Переменная окружения с адресом эндпоинта: её читает шлюз, а Claude и консоль её не получают.
pub const URL_ENV: &str = "CTP_FETCH_URL";

/// Ожидание одним куском: обычная страница — секунды; проверка с галочкой — сколько
/// человек сидит у окна (внутри `webfetch` свой срок и таймауты).
const TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Clone)]
struct Fetching {
    show: bool,
}

/// Живой ли последний запущенный сервер: перезапуск, если он умер. Адрес общий с шлюзом
/// (`CTP_FETCH_URL` обновляется на месте), поэтому тот продолжает ходить по новому порту.
/// Бывает нужно после того, как окно WebView2 уронило процесс.
pub fn spawn_watchdog() {
    std::thread::spawn(|| {
        loop {
            std::thread::sleep(Duration::from_secs(60));
            let Some(address) = current_address() else {
                continue;
            };
            if std::net::TcpStream::connect_timeout(&address, Duration::from_secs(2)).is_ok() {
                continue;
            }
            // SAFETY: адрес читают только потоки, созданные после этой записи.
            match serve(true) {
                Ok(url) => {
                    tracing::warn!(
                        address = address_of(&url),
                        "browser fetch endpoint restarted"
                    );
                    unsafe { std::env::set_var("CTP_FETCH_URL", &url) };
                }
                Err(exc) => tracing::warn!(error = %exc, "browser fetch endpoint not restarted"),
            }
        }
    });
}

/// Адрес для проверки живости: порт из `CTP_FETCH_URL`.
fn current_address() -> Option<std::net::SocketAddr> {
    let url = std::env::var("CTP_FETCH_URL").ok()?;
    let host = url.trim_start_matches("http://").split('/').next()?;
    host.parse().ok()
}

/// Сервер на 127.0.0.1:0, адрес которого отдаётся шлюзу. Живёт до конца приложения.
pub fn serve(show: bool) -> Result<String, std::io::Error> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    // `tokio::net::TcpListener::from_std` ждёт неблокирующий сокет: с блокирующим
    // accept виснет, и запросы к `/fetch` не получают ответа вовсе.
    listener.set_nonblocking(true)?;
    let port = listener.local_addr()?.port();
    // Порт случайный, но страница в браузере может перебрать порты: адрес несёт секрет
    // (его знает только шлюз, из `CTP_FETCH_URL`), а запросы со страниц отсекает `handle`.
    let secret = uuid::Uuid::new_v4().simple().to_string();
    let router = Router::new()
        .route(&format!("/fetch/{secret}"), post(handle))
        .with_state(Fetching { show });
    // Свой поток с циклом tokio: `setup` у Tauri не живёт внутри асинхронного рантайма.
    let started = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("fetch-endpoint".into())
        .spawn(move || {
            let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                let _ = started.0.send(Err(std::io::Error::other("tokio runtime")));
                return;
            };
            let _ = started.0.send(Ok(()));
            let _ = rt.block_on(async move {
                let listener = tokio::net::TcpListener::from_std(listener)?;
                if let Err(exc) = axum::serve(listener, router).await {
                    tracing::warn!(error = %exc, "the fetch server stopped with an error");
                }
                Ok::<(), std::io::Error>(())
            });
        })?;
    let _ = started.1.recv();
    Ok(format!("http://127.0.0.1:{port}/fetch/{secret}"))
}

/// Только адрес без секрета для журнала: сам секрет в журнал не пишется.
pub fn address_of(url: &str) -> &str {
    url.split('/').nth(2).unwrap_or("")
}

async fn handle(State(fetching): State<Fetching>, headers: HeaderMap, body: Bytes) -> Response {
    // Браузер всегда добавляет `Origin` или `Sec-Fetch-Site`; шлюз (reqwest) — нет.
    if headers.contains_key("origin") || headers.contains_key("sec-fetch-site") {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "not for web pages"})),
        )
            .into_response();
    }
    let Some(url) = wanted_url(&body) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "url is required"})),
        )
            .into_response();
    };
    let show = fetching.show;
    // Окно WebView2 — на своём потоке, как `upkeep` зовёт перевод: в пуле tokio сборка окна
    // может ждать главный поток бесконечно.
    let started = std::time::Instant::now();
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let target = url.clone();
    std::thread::spawn(move || {
        let _ = sender.send(crate::webfetch::fetch_html(&target, show));
    });
    let result = match tokio::time::timeout(TIMEOUT, receiver).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err(anyhow::anyhow!("the browser thread stopped")),
        Err(_) => {
            tracing::warn!(url = %url, "browser fetch timed out");
            return (
                StatusCode::GATEWAY_TIMEOUT,
                Json(json!({"error": "the browser did not finish in time"})),
            )
                .into_response();
        }
    };
    match result {
        Ok(html) => {
            tracing::info!(
                url = %url,
                ms = started.elapsed().as_millis(),
                bytes = html.len(),
                "fetched through the built-in browser"
            );
            (StatusCode::OK, html).into_response()
        }
        Err(exc) => {
            tracing::warn!(url = %url, error = %format!("{exc:#}"), "browser fetch failed");
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": format!("{exc:#}")})),
            )
                .into_response()
        }
    }
}

fn wanted_url(body: &[u8]) -> Option<String> {
    let value: Value = serde_json::from_slice(body).ok()?;
    let url = value.get("url")?.as_str()?.trim();
    // Только обычный веб: загружаем страницу, а не файл на диске и не служебную схему.
    let ok = ["https://", "http://"]
        .iter()
        .any(|prefix| url.starts_with(prefix));
    (ok && url.len() > 8).then(|| url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn only_the_gateway_with_the_secret_may_ask_for_a_page() {
        // The port is random but a page in the browser can scan ports: the address carries a
        // secret, and anything that comes from a web page (it has an Origin) is refused anyway.
        let url = serve(false).unwrap();
        let secret = url.rsplit('/').next().unwrap();
        assert!(secret.len() >= 32, "{url}");
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let bare = url.rsplit_once('/').unwrap().0;
        let status = |request: reqwest::RequestBuilder| async move {
            request.body("{}").send().await.unwrap().status().as_u16()
        };
        assert_eq!(status(client.post(bare)).await, 404);
        assert_eq!(
            status(client.post(&url).header("origin", "https://evil.example")).await,
            403
        );
        // The gateway itself: no Origin, the right address; no `url` in the body is a 400.
        assert_eq!(status(client.post(&url)).await, 400);
    }

    #[test]
    fn only_web_addresses_are_fetched() {
        assert_eq!(
            wanted_url(br#"{"url":"https://search.brave.com/search?q=a"}"#).as_deref(),
            Some("https://search.brave.com/search?q=a")
        );
        assert_eq!(wanted_url(br#"{"url":"file:///C:/Windows/win.ini"}"#), None);
        assert_eq!(wanted_url(br#"{"url":"javascript:alert(1)"}"#), None);
        assert_eq!(wanted_url(br#"{}"#), None);
        assert_eq!(wanted_url(b"not json"), None);
    }
}
