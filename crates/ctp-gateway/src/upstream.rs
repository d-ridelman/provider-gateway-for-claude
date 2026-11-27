use crate::error;
use crate::passthrough::bytes_response;
use crate::state::Provider;
use axum::body::Bytes;
use axum::http::{HeaderMap, HeaderValue};
use axum::response::Response;

/// Отказ провайдера до первого байта ответа. Вид отказа нужен обработчику, чтобы решить,
/// повторять ли запрос у запасной модели (ADR-023), а собранный `Response` для этого не годится:
/// в нём статус уже переписан классификацией.
pub(crate) enum Failure {
    Network(String),
    Status {
        status: u16,
        text: String,
        retry_after: Option<String>,
        /// Ответ как есть, если сквозной путь сегодня отдал бы его байтами.
        raw: Option<Box<Raw>>,
    },
    /// `2xx`, тело которого не прочиталось: провайдер отвечает, но ответом не является.
    Body(String),
}

pub(crate) struct Raw {
    pub status: u16,
    pub content_type: String,
    pub payload: Bytes,
    pub headers: HeaderMap,
}

impl Failure {
    /// Повторяем у запасной: сеть, таймаут, 402, 429, 5xx (после классификации) или
    /// непрочитанное тело. Отказы запроса (400, 401, 403, 404, 413, 422) — не повторяем.
    pub(crate) fn retryable(&self) -> bool {
        match self {
            Self::Network(_) | Self::Body(_) => true,
            Self::Status { status, text, .. } => {
                matches!(error::classify(*status, text, "").0, 402 | 429 | 500..=599)
            }
        }
    }

    /// Короткая причина для журнала: без текста провайдера, значит и без ключа.
    pub(crate) fn label(&self) -> String {
        match self {
            Self::Network(_) => "network".into(),
            Self::Body(_) => "unreadable body".into(),
            Self::Status { status, text, .. } => {
                format!("status {}", error::classify(*status, text, "").0)
            }
        }
    }

    /// Ответ клиенту, когда запасной нет или не помогла и она: отказ основной целиком,
    /// как без запасной.
    pub(crate) fn response(self, provider: &Provider) -> Response {
        match self {
            Self::Network(text) => error::anthropic_error(502, "api_error", text),
            Self::Body(text) => error::anthropic_error(502, "api_error", text),
            Self::Status {
                status,
                text,
                retry_after,
                raw,
            } => {
                if let Some(raw) = raw {
                    let mut response = bytes_response(raw.status, &raw.content_type, raw.payload);
                    response.headers_mut().extend(raw.headers);
                    return response;
                }
                let mut response = error::upstream_error(status, &text, &provider.api_key);
                if let Some(value) = retry_after
                    && let Ok(value) = HeaderValue::from_str(&value)
                {
                    response.headers_mut().insert("retry-after", value);
                }
                response
            }
        }
    }
}
