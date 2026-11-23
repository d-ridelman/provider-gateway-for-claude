use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use regex::Regex;
use serde_json::{Value, json};
use std::sync::OnceLock;

pub fn error_value(kind: &str, message: impl Into<String>) -> Value {
    json!({"type": "error", "error": {"type": kind, "message": message.into()}})
}

/// Ответ в форме Anthropic Messages API.
pub fn anthropic_error(status: u16, kind: &str, message: impl Into<String>) -> Response {
    let status = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
    (status, Json(error_value(kind, message))).into_response()
}

/// `secret` — ключ провайдера: его вырезаем из сырого текста до regex и обрезки,
/// иначе они разрежут ключ необычной формы и хвост уйдёт клиенту.
pub fn upstream_error(status: u16, text: &str, secret: &str) -> Response {
    let (code, kind, message) = classify(status, text, secret);
    anthropic_error(code, kind, message)
}

/// Ошибка провайдера как её отдаёт Anthropic: статус, тип, сообщение. Общая для ответа
/// и для ошибки внутри уже начатого потока.
pub fn classify(status: u16, text: &str, secret: &str) -> (u16, &'static str, String) {
    let code = if (400..600).contains(&status) {
        status
    } else {
        502
    };
    let message = clean_error_text(text, secret);
    let provider_said = crate::openai::adapt::error_message(text);
    if matches!(code, 400 | 413 | 422) && crate::openai::adapt::is_overflow(text) {
        // Так отвечает Anthropic, когда разговор не влез в окно, и Claude Code по этому
        // тексту сам сжимает его и повторяет, а не показывает ошибку. Числа он достаёт
        // из «N tokens > M maximum»: берём их из текста провайдера, если они там есть.
        let said = crate::openai::adapt::overflow_message(&provider_said)
            .unwrap_or_else(|| format!("prompt is too long: {message}"));
        return (400, "invalid_request_error", said);
    }
    (code, error_kind(code), message)
}

/// Тип ошибки Anthropic по статусу: по нему Claude Code решает, повторять ли запрос
/// (лимит, перегрузка) и что показать.
fn error_kind(status: u16) -> &'static str {
    match status {
        400 | 422 => "invalid_request_error",
        401 => "authentication_error",
        403 => "permission_error",
        404 => "not_found_error",
        413 => "request_too_large",
        429 => "rate_limit_error",
        529 => "overloaded_error",
        _ => "api_error",
    }
}

/// Текст ошибки провайдера для клиента: ключ вырезан и из сырого тела, и из
/// разобранного сообщения (в JSON он мог быть экранирован: `\/`, `+`),
/// и только потом regex и обрезка.
pub fn clean_error_text(text: &str, secret: &str) -> String {
    let text = without_secret(text, secret);
    let parsed: Option<Value> = serde_json::from_str(text.trim()).ok();
    // JSON без `error.message` пересериализуем: serde раскрывает `\/` и `\uXXXX`,
    // и экранированный ключ снова совпадает с настоящим.
    let message = match &parsed {
        Some(value) => openai_error_message(value).unwrap_or_else(|| value.to_string()),
        None => text,
    };
    truncate_redacted(&without_secret(&message, secret))
}

/// Убирает сам ключ из текста (короткие «ключи» не трогаем — заменили бы полтекста).
pub fn without_secret(text: &str, secret: &str) -> String {
    if secret.len() >= 4 {
        text.replace(secret, "…")
    } else {
        text.to_string()
    }
}

/// То же в байтах: тело не в UTF-8 (HTML прокси в cp1251) остаётся нетронутым, а ключ из него
/// вырезается. Ключ ASCII, его байты находятся и в такой кодировке.
pub fn without_secret_bytes(body: &[u8], secret: &str) -> Vec<u8> {
    let needle = secret.as_bytes();
    if needle.len() < 4 {
        return body.to_vec();
    }
    let mut out = Vec::with_capacity(body.len());
    let mut at = 0;
    while at < body.len() {
        if body[at..].starts_with(needle) {
            out.extend_from_slice("…".as_bytes());
            at += needle.len();
        } else {
            out.push(body[at]);
            at += 1;
        }
    }
    out
}

/// Достаёт человеческий текст из OpenAI- или Anthropic-ошибки, иначе режет сырой текст.
pub fn message_from_error_text(text: &str) -> String {
    clean_error_text(text, "")
}

pub fn openai_error_message(value: &Value) -> Option<String> {
    let error = value.get("error")?;
    error
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| error.as_str())
        .map(str::to_string)
}

pub fn truncate_redacted(text: &str) -> String {
    let text = text.trim();
    let text = if text.is_empty() {
        "upstream error".to_string()
    } else {
        redact(text)
    };
    text.chars().take(2000).collect()
}

fn redact(text: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"(?i)(sk-[a-z0-9._-]{6,}|bearer\s+[a-z0-9._-]{6,})").expect("redaction regex")
    });
    re.replace_all(text, |caps: &regex::Captures<'_>| {
        if caps[0].to_ascii_lowercase().starts_with("bearer") {
            "Bearer …".to_string()
        } else {
            "sk-…".to_string()
        }
    })
    .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classified_errors_carry_anthropics_status_kind_and_overflow_numbers() {
        let (status, kind, message) = classify(429, r#"{"error":{"message":"slow"}}"#, "");
        assert_eq!(
            (status, kind, message.as_str()),
            (429, "rate_limit_error", "slow")
        );
        assert_eq!(classify(529, "busy", "").1, "overloaded_error");
        assert_eq!(classify(99, "weird", "").0, 502);
        let overflow = r#"{"error":{"message":"This model's maximum context length is 8192 tokens. However, your messages resulted in 9000 tokens."}}"#;
        let (status, kind, message) = classify(400, overflow, "");
        assert_eq!((status, kind), (400, "invalid_request_error"));
        assert_eq!(message, "prompt is too long: 9000 tokens > 8192 maximum");
        // Без чисел в тексте провайдера — его слова после стандартного начала.
        let (_, _, message) = classify(413, "context window exceeded", "");
        assert_eq!(message, "prompt is too long: context window exceeded");
        // Предел вывода — не переполнение окна.
        let (_, _, message) = classify(400, "max_tokens exceeds the output token limit", "");
        assert!(!message.starts_with("prompt is too long"));
    }

    #[test]
    fn redacts_keys_and_truncates() {
        let text = format!(
            "bad key sk-abcdefghij and Bearer abcdefghij {}",
            "x".repeat(3000)
        );
        let out = truncate_redacted(&text);
        assert!(!out.contains("sk-abcdefghij"));
        assert!(!out.contains("abcdefghij"));
        assert_eq!(out.chars().count(), 2000);
    }

    #[test]
    fn extracts_openai_message() {
        let text = r#"{"error":{"message":"model does not exist","type":"invalid_request_error"}}"#;
        assert_eq!(message_from_error_text(text), "model does not exist");
    }

    #[test]
    fn escaped_key_outside_error_message_is_removed() {
        let key = "sk-abcdef+ghi/jkl";
        for body in [
            r#"{"detail":"bad key sk-abcdef+ghi\/jkl"}"#,
            r#"{"detail":"bad key sk-abcdef+ghi/jkl"}"#,
        ] {
            let out = clean_error_text(body, key);
            assert!(!out.contains("ghi") && !out.contains("abcdef"), "{out}");
        }
    }
}
