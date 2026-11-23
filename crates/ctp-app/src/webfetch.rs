//! Скачивание через встроенный браузер (WebView2) — для страниц, которые Cloudflare
//! закрывает проверкой на бота для программ (`cf-mitigated: challenge`): браузер
//! проходит её сам, как у обычного посетителя. Если проверка просит нажать галочку,
//! окно через несколько секунд показывается, и человек нажимает сам.
//!
//! Окно открывает только внешний адрес и прав на команды приложения не получает:
//! capability `default` выдана одному окну `main`.
// В тестах настоящий `fetch_json` выключен (см. ниже) — его импорты там не нужны.
#![cfg_attr(test, allow(unused_imports, dead_code))]

use crate::tr;
use anyhow::{Context, Result, bail};
use std::sync::OnceLock;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

static APP: OnceLock<tauri::AppHandle> = OnceLock::new();

const LABEL: &str = "webfetch";
/// Прошла ли проверка сама — видно за пару секунд; дольше — показываем окно.
const SHOW_AFTER: Duration = Duration::from_secs(8);
const TIMEOUT: Duration = Duration::from_secs(120);
/// Без окна (фон) ждём дольше, чем до показа окна: медленная загрузка 6 МБ не должна
/// сойти за проверку, которая просит галочку.
const HIDDEN_LIMIT: Duration = Duration::from_secs(30);

/// Страница с JSON (после проверки) — её текст, иначе `null`: проверка ещё идёт.
const SCRIPT: &str = "(() => { if (document.contentType !== 'application/json') return null; \
    const pre = document.querySelector('pre'); \
    return pre ? pre.textContent : document.body.innerText; })()";

/// Из `setup`: без этого `fetch_json` сразу отвечает ошибкой (тесты, утилиты).
pub fn init(app: &tauri::AppHandle) {
    let _ = APP.set(app.clone());
}

/// Ответ `eval_with_callback` — результат скрипта в JSON: строка или `null`.
fn page_text(result: &str) -> Option<String> {
    serde_json::from_str::<Option<String>>(result)
        .ok()
        .flatten()
        .filter(|text| !text.is_empty())
}

/// В тестах окна нет: создание окна тянет comctl32 v6, а у тестового exe нет манифеста
/// приложения — он бы не запустился вовсе (STATUS_ENTRYPOINT_NOT_FOUND).
#[cfg(test)]
pub fn fetch_json(_url: &str, _show: bool) -> Result<String> {
    bail!("no built-in browser in tests")
}

/// HTML всей страницы: для поиска нужен её текст и ссылки, а не ответ API.
const SCRIPT_HTML: &str = "(() => document.contentType === 'text/html' \
    ? document.documentElement.outerHTML : null)()";

/// HTML страницы глазами браузера — для браузерного прохода проверки на бота в поиске.
/// Отличается от `fetch_json` только тем, что отдаёт обычный HTML, а не требует JSON:
/// окно по-прежнему видно, только если проверка не прошла сама.
#[cfg(not(test))]
pub fn fetch_html(url: &str, show: bool) -> Result<String> {
    let app = APP.get().context(tr!(
        "встроенный браузер недоступен",
        "built-in browser is not available"
    ))?;
    // Своё окно на каждый поиск: Claude Code шлёт несколько поисков разом, и общая метка
    // заставляла бы каждый новый уничтожать окно соседа.
    let label = format!("webfetch-search-{}", uuid::Uuid::new_v4().simple());
    let label = label.as_str();
    let window = WebviewWindowBuilder::new(app, label, WebviewUrl::External(url.parse()?))
        .title(tr!("Поиск — проверка", "Search — check"))
        .inner_size(720.0, 780.0)
        .center()
        .visible(false)
        .build()?;
    struct Close(tauri::WebviewWindow);
    impl Drop for Close {
        fn drop(&mut self) {
            let _ = self.0.destroy();
        }
    }
    let window = Close(window);

    let started = Instant::now();
    let mut shown = false;
    loop {
        if app.get_webview_window(label).is_none() {
            bail!(
                "{}",
                tr!("окно поиска закрыто", "the search window was closed")
            );
        }
        if started.elapsed() > TIMEOUT {
            bail!(
                "{}",
                tr!(
                    "проверка поисковика не пройдена за 2 минуты",
                    "the search engine's check was not passed within 2 minutes"
                )
            );
        }
        if !show && started.elapsed() > HIDDEN_LIMIT {
            bail!(
                "{}",
                tr!(
                    "поисковик просит подтверждения — фоном не пройти",
                    "the search engine asks for confirmation; cannot pass it in the background"
                )
            );
        }
        if show && !shown && started.elapsed() > SHOW_AFTER {
            let _ = window.0.show();
            let _ = window.0.set_focus();
            shown = true;
        }
        let (tx, rx) = mpsc::channel();
        window.0.eval_with_callback(SCRIPT_HTML, move |result| {
            let _ = tx.send(result);
        })?;
        if let Ok(result) = rx.recv_timeout(Duration::from_secs(10))
            && let Some(text) = page_text(&result)
        {
            return Ok(text);
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

#[cfg(test)]
pub fn fetch_html(_url: &str, _show: bool) -> Result<String> {
    bail!("no built-in browser in tests")
}

/// Текст JSON по `url` глазами браузера. Блокирует: вызывать не из главного потока.
/// `show` — можно показать окно, если проверка не прошла сама (человек рядом). Без него
/// (фоновая подготовка) окно не появляется: не прошла за `HIDDEN_LIMIT` — отказ.
#[cfg(not(test))]
pub fn fetch_json(url: &str, show: bool) -> Result<String> {
    let app = APP.get().context(tr!(
        "встроенный браузер недоступен",
        "built-in browser is not available"
    ))?;
    if let Some(stale) = app.get_webview_window(LABEL) {
        let _ = stale.destroy();
    }
    let window = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::External(url.parse()?))
        .title(tr!(
            "Provider Gateway for Claude — загрузка перевода",
            "Provider Gateway for Claude — downloading the translation"
        ))
        .inner_size(560.0, 640.0)
        .center()
        .visible(false)
        .build()?;
    // Окно не переживает загрузку ни при каком исходе.
    struct Close(tauri::WebviewWindow);
    impl Drop for Close {
        fn drop(&mut self) {
            let _ = self.0.destroy();
        }
    }
    let window = Close(window);

    let started = Instant::now();
    let mut shown = false;
    loop {
        if app.get_webview_window(LABEL).is_none() {
            bail!(
                "{}",
                tr!("окно загрузки закрыто", "the download window was closed")
            );
        }
        if started.elapsed() > TIMEOUT {
            bail!(
                "{}",
                tr!(
                    "проверка claude.ai не пройдена за 2 минуты",
                    "claude.ai check not passed within 2 minutes"
                )
            );
        }
        if !show && started.elapsed() > HIDDEN_LIMIT {
            bail!(
                "{}",
                tr!(
                    "claude.ai просит подтверждения — фоном не скачать",
                    "claude.ai asks for confirmation; cannot download in the background"
                )
            );
        }
        if show && !shown && started.elapsed() > SHOW_AFTER {
            let _ = window.0.show();
            let _ = window.0.set_focus();
            shown = true;
        }
        let (tx, rx) = mpsc::channel();
        window.0.eval_with_callback(SCRIPT, move |result| {
            let _ = tx.send(result);
        })?;
        if let Ok(result) = rx.recv_timeout(Duration::from_secs(10))
            && let Some(text) = page_text(&result)
        {
            return Ok(text);
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_result_is_text_only_when_the_page_is_json() {
        assert_eq!(page_text(r#""{\"a\":1}""#).as_deref(), Some(r#"{"a":1}"#));
        assert_eq!(page_text("null"), None);
        assert_eq!(page_text(r#""""#), None);
        assert_eq!(page_text(""), None); // exception or no result
    }
}
