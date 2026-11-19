use axum::body::Body;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use futures_util::stream;
use serde_json::Value;
use std::convert::Infallible;

/// Одно SSE-событие Anthropic: имя берётся из поля `type`.
pub fn frame(value: &Value) -> Bytes {
    let name = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("message");
    let data = serde_json::to_string(value).unwrap_or_else(|_| "{}".to_string());
    Bytes::from(format!("event: {name}\ndata: {data}\n\n"))
}

/// Готовый SSE-ответ из уже собранных событий (probe, не-потоковый fallback).
pub fn response(events: Vec<Value>) -> Response {
    let body = Body::from_stream(stream::iter(
        events
            .into_iter()
            .map(|value| Ok::<Bytes, Infallible>(frame(&value))),
    ));
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(body)
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}
