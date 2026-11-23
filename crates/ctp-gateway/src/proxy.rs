//! Исходящий прокси шлюза и приложения (обычно mixed-порт mihomo): один адрес на процесс.
//!
//! Задаётся до запуска шлюза; клиенты, собранные раньше, остаются со старым адресом,
//! поэтому приложение перезапускает шлюз при смене прокси.

use std::sync::RwLock;

/// Мимо прокси: сам компьютер, локальные сети и имена локальной сети (Ollama, LM Studio).
/// shortcut: имя без точки (`gpu-box`) и корпоративный домен идут через VPN — указывайте
/// такой сервер по IP; добавить список своих доменов в окно, если попросят.
pub const NO_PROXY: &str = "localhost,127.0.0.1,::1,10.0.0.0/8,172.16.0.0/12,192.168.0.0/16,\
    169.254.0.0/16,.local,.lan,.internal,.home.arpa,.localdomain";

static CURRENT: RwLock<Option<String>> = RwLock::new(None);

/// `Some("http://127.0.0.1:7890")` — ходить через прокси; `None` — как настроена система.
pub fn set(url: Option<String>) {
    *CURRENT.write().unwrap_or_else(|e| e.into_inner()) = url;
}

pub fn current() -> Option<String> {
    CURRENT.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Добавляет прокси к сборщику клиента, если он задан.
pub fn apply(builder: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
    let Some(url) = current() else {
        return builder;
    };
    match reqwest::Proxy::all(&url) {
        Ok(proxy) => builder.proxy(proxy.no_proxy(reqwest::NoProxy::from_string(NO_PROXY))),
        Err(exc) => {
            tracing::warn!(%url, error = %exc, "bad proxy address, going direct");
            builder
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::path;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn outside_goes_through_the_proxy_and_loopback_does_not() {
        let proxy = MockServer::start().await;
        Mock::given(path("/x"))
            .respond_with(ResponseTemplate::new(200).set_body_string("via proxy"))
            .mount(&proxy)
            .await;
        let direct = MockServer::start().await;
        Mock::given(path("/x"))
            .respond_with(ResponseTemplate::new(200).set_body_string("direct"))
            .mount(&direct)
            .await;
        set(Some(proxy.uri()));
        let client = apply(reqwest::Client::builder()).build().unwrap();
        set(None);
        let far = client.get("http://example.invalid/x").send().await.unwrap();
        assert_eq!(far.text().await.unwrap(), "via proxy");
        let near = client
            .get(format!("{}/x", direct.uri()))
            .send()
            .await
            .unwrap();
        assert_eq!(near.text().await.unwrap(), "direct");
    }
}
