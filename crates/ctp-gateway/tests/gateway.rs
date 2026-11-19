use axum::body::Body;
use axum::http::{Request, StatusCode};
use ctp_core::config::ProvidersFile;
use ctp_gateway::{app, state::AppState, state::ModelRouter};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn state_with(base_url: &str, kind: &str, model: &str) -> AppState {
    let config = json!({
        "providers": [{
            "id": "p",
            "name": "P",
            "type": kind,
            "base_url": base_url,
            "api_key": "sk-upstream",
            "models": {"claude-p-00000000": model}
        }]
    });
    let file: ProvidersFile = serde_json::from_value(config).unwrap();
    let router = ModelRouter::new(file).unwrap();
    AppState::new(router, "local-token".into(), false, "user:inference".into()).unwrap()
}

fn post_messages(body: Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/v1/messages")
        .header("x-api-key", "local-token")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn json_body(response: axum::response::Response) -> Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

async fn text_body(response: axum::response::Response) -> String {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[tokio::test]
async fn non_stream_openai_is_translated() {
    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header("authorization", "Bearer sk-upstream"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "chatcmpl-1",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "hello"},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 7, "completion_tokens": 3}
        })))
        .mount(&upstream)
        .await;

    let state = state_with(&upstream.uri(), "openai", "gpt-5.2");
    let response = app(state)
        .oneshot(post_messages(json!({
            "model": "claude-p-00000000",
            "max_tokens": 32,
            "messages": [{"role": "user", "content": "hi"}]
        })))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["type"], "message");
    assert_eq!(body["content"][0]["type"], "text");
    assert_eq!(body["content"][0]["text"], "hello");
    assert_eq!(body["stop_reason"], "end_turn");
    assert_eq!(body["usage"]["input_tokens"], 7);
}

#[tokio::test]
async fn stream_openai_becomes_anthropic_sse() {
    let upstream = MockServer::start().await;
    let sse = concat!(
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"he\"},\"finish_reason\":null}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"llo\"},\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n"
    );
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse, "text/event-stream"),
        )
        .mount(&upstream)
        .await;

    let state = state_with(&upstream.uri(), "openai", "gpt-5.2");
    let response = app(state)
        .oneshot(post_messages(json!({
            "model": "claude-p-00000000",
            "max_tokens": 32,
            "stream": true,
            "messages": [{"role": "user", "content": "hi"}]
        })))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "text/event-stream"
    );
    let body = text_body(response).await;
    assert!(body.contains("event: message_start"));
    assert!(body.contains("\"type\":\"text_delta\""));
    assert!(body.contains("\"text\":\"he\""));
    assert!(body.contains("\"text\":\"llo\""));
    assert!(body.contains("\"stop_reason\":\"end_turn\""));
    assert!(body.contains("event: message_stop"));
}

#[tokio::test]
async fn probe_is_answered_without_upstream() {
    let state = state_with("http://127.0.0.1:9", "openai", "gpt-5.2");
    let response = app(state)
        .oneshot(post_messages(json!({
            "model": "claude-p-00000000",
            "max_tokens": 1,
            "messages": [{"role": "user", "content": "."}]
        })))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["content"][0]["text"], ".");
    assert_eq!(body["stop_reason"], "max_tokens");
}

#[tokio::test]
async fn anthropic_provider_passes_body_through() {
    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "sk-upstream"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "msg_x",
            "type": "message",
            "role": "assistant",
            "model": "claude-sonnet-4-5",
            "content": [{"type": "text", "text": "passthrough"}],
            "stop_reason": "end_turn",
            "stop_sequence": null,
            "usage": {"input_tokens": 1, "output_tokens": 1}
        })))
        .mount(&upstream)
        .await;

    let state = state_with(&upstream.uri(), "anthropic", "claude-sonnet-4-5");
    let response = app(state)
        .oneshot(post_messages(json!({
            "model": "claude-p-00000000",
            "max_tokens": 32,
            "messages": [{"role": "user", "content": "hi"}]
        })))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["content"][0]["text"], "passthrough");
}

#[tokio::test]
async fn auth_is_enforced() {
    let upstream = MockServer::start().await;
    let state = state_with(&upstream.uri(), "openai", "gpt-5.2");
    let request = Request::builder()
        .method("POST")
        .uri("/v1/messages")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "model": "claude-p-00000000",
                "max_tokens": 8,
                "messages": [{"role": "user", "content": "hi"}]
            })
            .to_string(),
        ))
        .unwrap();
    let response = app(state).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body = json_body(response).await;
    assert_eq!(body["type"], "error");
}

#[tokio::test]
async fn stream_openai_tool_call_becomes_tool_use_block() {
    let upstream = MockServer::start().await;
    let sse = concat!(
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"read\",\"arguments\":\"\"}}]},\"finish_reason\":null}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"path\\\":\\\"a.txt\\\"}\"}}]},\"finish_reason\":null}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
        "data: [DONE]\n\n"
    );
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse, "text/event-stream"),
        )
        .mount(&upstream)
        .await;

    let state = state_with(&upstream.uri(), "openai", "gpt-5.2");
    let response = app(state)
        .oneshot(post_messages(json!({
            "model": "claude-p-00000000",
            "max_tokens": 32,
            "stream": true,
            "messages": [{"role": "user", "content": "read a.txt"}]
        })))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = text_body(response).await;
    assert!(body.contains("\"type\":\"tool_use\""));
    assert!(body.contains("\"id\":\"call_1\""));
    assert!(body.contains("\"name\":\"read\""));
    assert!(body.contains("\"partial_json\":\"{\\\"path\\\":\\\"a.txt\\\"}\""));
    assert!(body.contains("\"stop_reason\":\"tool_use\""));
}

#[tokio::test]
async fn anthropic_stream_passes_raw_sse() {
    let upstream = MockServer::start().await;
    let sse = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_x\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-sonnet-4-5\",\"content\":[],\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"raw\"}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n"
    );
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "sk-upstream"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse, "text/event-stream"),
        )
        .mount(&upstream)
        .await;

    let state = state_with(&upstream.uri(), "anthropic", "claude-sonnet-4-5");
    let response = app(state)
        .oneshot(post_messages(json!({
            "model": "claude-p-00000000",
            "max_tokens": 32,
            "stream": true,
            "messages": [{"role": "user", "content": "hi"}]
        })))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = text_body(response).await;
    assert!(body.contains("event: message_start"));
    assert!(body.contains("\"text\":\"raw\""));
    assert!(body.contains("event: message_stop"));
}

#[tokio::test]
async fn retries_once_with_smaller_max_tokens() {
    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error": {
                "message": "max_tokens: 4096 is greater than the maximum allowed 1024",
                "type": "invalid_request_error"
            }
        })))
        .up_to_n_times(1)
        .mount(&upstream)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "chatcmpl-2",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "ok"},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 5, "completion_tokens": 1}
        })))
        .mount(&upstream)
        .await;

    let state = state_with(&upstream.uri(), "openai", "gpt-5.2");
    let response = app(state)
        .oneshot(post_messages(json!({
            "model": "claude-p-00000000",
            "max_tokens": 4096,
            "messages": [{"role": "user", "content": "hi"}]
        })))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["content"][0]["text"], "ok");
    assert_eq!(upstream.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn anthropic_passthrough_keeps_headers_and_body() {
    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "sk-upstream"))
        .and(header("anthropic-beta", "context-1m-2025-08-07"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "msg_x",
            "type": "message",
            "role": "assistant",
            "model": "claude-test",
            "content": [{"type": "thinking", "thinking": "размышление"}],
            "stop_reason": "end_turn",
            "stop_sequence": null,
            "usage": {"input_tokens": 5, "output_tokens": 3}
        })))
        .mount(&upstream)
        .await;

    let state = state_with(&upstream.uri(), "anthropic", "claude-test");
    let request = Request::builder()
        .method("POST")
        .uri("/v1/messages")
        .header("x-api-key", "local-token")
        .header("anthropic-beta", "context-1m-2025-08-07")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "model": "claude-p-00000000",
                "max_tokens": 64,
                "messages": [{"role": "user", "content": "hi"}]
            })
            .to_string(),
        ))
        .unwrap();
    let response = app(state).oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["content"][0]["type"], "thinking");
    assert_eq!(body["content"][0]["thinking"], "размышление");

    let requests = upstream.received_requests().await.unwrap();
    let last = requests.last().unwrap();
    assert_eq!(last.headers.get("anthropic-version").unwrap(), "2023-06-01");
    assert_eq!(
        last.headers.get("anthropic-beta").unwrap(),
        "context-1m-2025-08-07"
    );
    let sent: Value = serde_json::from_slice(&last.body).unwrap();
    assert_eq!(sent["model"], "claude-test");
    assert_eq!(sent["stream"], false);
}

#[tokio::test]
async fn anthropic_count_tokens_uses_upstream_counter() {
    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages/count_tokens"))
        .and(header("x-api-key", "sk-upstream"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"input_tokens": 4242})))
        .mount(&upstream)
        .await;

    let state = state_with(&upstream.uri(), "anthropic", "claude-test");
    let request = Request::builder()
        .method("POST")
        .uri("/v1/messages/count_tokens")
        .header("x-api-key", "local-token")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "model": "claude-p-00000000",
                "messages": [{"role": "user", "content": "x"}]
            })
            .to_string(),
        ))
        .unwrap();
    let response = app(state).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["input_tokens"], 4242);
}

#[tokio::test]
async fn openai_count_tokens_is_local() {
    let state = state_with("http://127.0.0.1:9", "openai", "gpt-5.2");
    let request = Request::builder()
        .method("POST")
        .uri("/v1/messages/count_tokens")
        .header("x-api-key", "local-token")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "model": "claude-p-00000000",
                "messages": [{"role": "user", "content": "hello"}]
            })
            .to_string(),
        ))
        .unwrap();
    let response = app(state).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert!(body["input_tokens"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn tools_are_forwarded_to_openai() {
    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "chatcmpl-3",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "ok"},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 5, "completion_tokens": 1}
        })))
        .mount(&upstream)
        .await;

    let state = state_with(&upstream.uri(), "openai", "gpt-5.2");
    let response = app(state)
        .oneshot(post_messages(json!({
            "model": "claude-p-00000000",
            "max_tokens": 32,
            "tools": [{
                "name": "ping",
                "description": "ping",
                "input_schema": {"type": "object", "properties": {}}
            }],
            "messages": [{"role": "user", "content": "ping"}]
        })))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let requests = upstream.received_requests().await.unwrap();
    let sent: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(sent["tools"][0]["type"], "function");
    assert_eq!(sent["tools"][0]["function"]["name"], "ping");
}

#[tokio::test]
async fn rate_limit_response_keeps_retry_after() {
    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "7")
                .set_body_json(json!({"error": {"message": "rate limited"}})),
        )
        .mount(&upstream)
        .await;

    let state = state_with(&upstream.uri(), "openai", "gpt-5.2");
    let response = app(state)
        .oneshot(post_messages(json!({
            "model": "claude-p-00000000",
            "max_tokens": 32,
            "messages": [{"role": "user", "content": "hi"}]
        })))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(response.headers().get("retry-after").unwrap(), "7");
    let body = json_body(response).await;
    assert_eq!(body["error"]["message"], "rate limited");
}

#[tokio::test]
async fn models_list_aliases() {
    let upstream = MockServer::start().await;
    let state = state_with(&upstream.uri(), "openai", "gpt-5.2");
    let request = Request::builder()
        .method("GET")
        .uri("/v1/models")
        .header("x-api-key", "local-token")
        .body(Body::empty())
        .unwrap();
    let response = app(state).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["data"][0]["id"], "claude-p-00000000");
    assert_eq!(body["has_more"], false);
}

fn provider_from(value: Value) -> ctp_gateway::state::Provider {
    let cfg: ctp_core::config::ProviderConfig = serde_json::from_value(value).unwrap();
    ctp_gateway::state::Provider::from_config(&cfg).unwrap()
}

#[tokio::test]
async fn probe_lists_openai_models_with_bearer() {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(header("authorization", "Bearer sk-upstream"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "object": "list",
            "data": [{"id": "gpt-5.2"}, {"id": "deepseek-v4"}]
        })))
        .mount(&upstream)
        .await;

    let provider = provider_from(json!({
        "id": "p", "type": "openai", "base_url": format!("{}/v1", upstream.uri()),
        "api_key": "sk-upstream"
    }));
    let models = ctp_gateway::probe::list_models(&provider).await.unwrap();
    assert_eq!(models, vec!["deepseek-v4", "gpt-5.2"]);
}

#[tokio::test]
async fn probe_lists_anthropic_models_with_limit_and_key() {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(wiremock::matchers::query_param("limit", "1000"))
        .and(header("x-api-key", "sk-ant-x"))
        .and(header("anthropic-version", "2023-06-01"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"data": [{"id": "claude-sonnet-4-5", "type": "model"}]})),
        )
        .mount(&upstream)
        .await;

    let provider = provider_from(json!({
        "id": "a", "type": "anthropic", "base_url": upstream.uri(), "api_key": "sk-ant-x"
    }));
    let models = ctp_gateway::probe::list_models(&provider).await.unwrap();
    assert_eq!(models, vec!["claude-sonnet-4-5"]);
}

#[tokio::test]
async fn probe_reports_auth_error_without_leaking_key() {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "error": {"message": "Incorrect API key provided: sk-wrongkey123", "type": "invalid_request_error"}
        })))
        .mount(&upstream)
        .await;

    let provider = provider_from(json!({
        "id": "p", "type": "openai", "base_url": upstream.uri(), "api_key": "sk-wrongkey123"
    }));
    let error = ctp_gateway::probe::list_models(&provider)
        .await
        .unwrap_err();
    assert!(error.starts_with("HTTP 401"), "{error}");
    assert!(!error.contains("sk-wrongkey123"), "{error}");
}

#[tokio::test]
async fn chat_check_goes_to_provider_not_local_probe() {
    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "OK"},
                         "finish_reason": "stop"}]
        })))
        .expect(1)
        .mount(&upstream)
        .await;

    let file: ProvidersFile = serde_json::from_value(json!({"providers": [{
        "id": "p", "type": "openai", "base_url": upstream.uri(), "api_key": "k",
        "models": {"claude-p-00000000": "gpt-5.2"}
    }]}))
    .unwrap();
    let reply = ctp_gateway::probe::chat_check(file, "claude-p-00000000")
        .await
        .unwrap();
    assert_eq!(reply, "OK");
}

#[tokio::test]
async fn chat_check_returns_provider_error() {
    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "error": {"message": "model gpt-x does not exist"}
        })))
        .mount(&upstream)
        .await;

    let file: ProvidersFile = serde_json::from_value(json!({"providers": [{
        "id": "p", "type": "openai", "base_url": upstream.uri(),
        "models": {"claude-p-00000000": "gpt-x"}
    }]}))
    .unwrap();
    let error = ctp_gateway::probe::chat_check(file, "claude-p-00000000")
        .await
        .unwrap_err();
    assert_eq!(error, "HTTP 404: model gpt-x does not exist");
}

const ODD_KEY: &str = "sk-abcdef+ghi/jkl";

#[tokio::test]
async fn list_models_never_leaks_part_of_an_odd_key() {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(header("authorization", "Bearer sk-abcdef+ghi/jkl"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "error": {"message": format!("Incorrect API key provided: {ODD_KEY}")}
        })))
        .mount(&upstream)
        .await;
    let provider = provider_from(json!({
        "id": "p", "type": "openai", "base_url": upstream.uri(), "api_key": ODD_KEY
    }));
    let error = ctp_gateway::probe::list_models(&provider)
        .await
        .unwrap_err();
    assert!(
        !error.contains("ghi/jkl") && !error.contains("abcdef"),
        "{error}"
    );

    // HTML вместо JSON, ключ на границе 200-символьной выдержки.
    let html = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(format!("<html>{}{ODD_KEY}</html>", "x".repeat(185))),
        )
        .mount(&html)
        .await;
    let provider = provider_from(json!({
        "id": "p", "type": "openai", "base_url": html.uri(), "api_key": ODD_KEY
    }));
    let error = ctp_gateway::probe::list_models(&provider)
        .await
        .unwrap_err();
    assert!(
        !error.contains("sk-abc") && !error.contains("ghi/"),
        "{error}"
    );

    // PHP и прочие экранируют `/` в JSON: сырой текст ключ целиком не содержит.
    let escaped = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(401).set_body_raw(
            r#"{"error":{"message":"Incorrect API key provided: sk-abcdef+ghi\/jkl"}}"#,
            "application/json",
        ))
        .mount(&escaped)
        .await;
    let provider = provider_from(json!({
        "id": "p", "type": "openai", "base_url": escaped.uri(), "api_key": ODD_KEY
    }));
    let error = ctp_gateway::probe::list_models(&provider)
        .await
        .unwrap_err();
    assert!(error.starts_with("HTTP 401"), "{error}");
    assert!(
        !error.contains("ghi") && !error.contains("abcdef"),
        "{error}"
    );
}

#[tokio::test]
async fn openai_error_path_never_leaks_part_of_an_odd_key() {
    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "error": {"message": format!("Incorrect API key provided: {ODD_KEY}")}
        })))
        .mount(&upstream)
        .await;
    let file: ProvidersFile = serde_json::from_value(json!({"providers": [{
        "id": "p", "type": "openai", "base_url": upstream.uri(), "api_key": ODD_KEY,
        "models": {"claude-p-00000000": "gpt-5.2"}
    }]}))
    .unwrap();
    let error = ctp_gateway::probe::chat_check(file, "claude-p-00000000")
        .await
        .unwrap_err();
    assert!(error.starts_with("HTTP 401"), "{error}");
    assert!(
        !error.contains("ghi/jkl") && !error.contains("abcdef"),
        "{error}"
    );
}

#[tokio::test]
async fn probe_explains_connection_refused() {
    // Порт 9 (discard) на локальной машине закрыт.
    let provider = provider_from(json!({
        "id": "p", "type": "openai", "base_url": "http://127.0.0.1:9/v1", "api_key": "k"
    }));
    let error = ctp_gateway::probe::list_models(&provider)
        .await
        .unwrap_err();
    assert!(error.contains("не удалось подключиться"), "{error}");
}

#[tokio::test]
async fn chat_check_scrubs_echoed_key_of_any_shape() {
    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "type": "error",
            "error": {"type": "authentication_error", "message": "invalid x-api-key AIzaSyEchoedKey42"}
        })))
        .mount(&upstream)
        .await;

    let file: ProvidersFile = serde_json::from_value(json!({"providers": [{
        "id": "a", "type": "anthropic", "base_url": upstream.uri(),
        "api_key": "AIzaSyEchoedKey42",
        "models": {"claude-a-00000000": "claude-sonnet-4-5"}
    }]}))
    .unwrap();
    let error = ctp_gateway::probe::chat_check(file, "claude-a-00000000")
        .await
        .unwrap_err();
    assert!(error.starts_with("HTTP 401"), "{error}");
    assert!(!error.contains("AIzaSyEchoedKey42"), "{error}");
}

#[tokio::test]
async fn anthropic_effort_is_clamped_and_defaulted_in_output_config() {
    let upstream = MockServer::start().await;
    let reply = json!({
        "id": "msg_1", "type": "message", "role": "assistant", "model": "claude-sonnet-4-6",
        "content": [{"type": "text", "text": "ok"}], "stop_reason": "end_turn",
        "stop_sequence": null, "usage": {"input_tokens": 1, "output_tokens": 1}
    });
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(wiremock::matchers::body_partial_json(
            json!({"output_config": {"effort": "high"}}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(reply.clone()))
        .expect(1)
        .mount(&upstream)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(wiremock::matchers::body_partial_json(
            json!({"output_config": {"effort": "medium"}}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(reply))
        .expect(1)
        .mount(&upstream)
        .await;

    let file: ProvidersFile = serde_json::from_value(json!({"providers": [{
        "id": "a", "type": "anthropic", "base_url": upstream.uri(), "api_key": "k",
        "models": {"claude-a-00000000": "claude-sonnet-4-6"},
        "max_effort": "high", "default_effort": "medium"
    }]}))
    .unwrap();
    let state = AppState::new(
        ModelRouter::new(file).unwrap(),
        "local-token".into(),
        false,
        "user:inference".into(),
    )
    .unwrap();
    let ask = |extra: Value| {
        let mut body = json!({
            "model": "claude-a-00000000", "max_tokens": 32,
            "messages": [{"role": "user", "content": "hi"}]
        });
        if let (Some(b), Some(e)) = (body.as_object_mut(), extra.as_object()) {
            b.extend(e.clone());
        }
        post_messages(body)
    };
    // max → обрезано до потолка high; без уровня → medium по умолчанию.
    let over = app(state.clone())
        .oneshot(ask(json!({"output_config": {"effort": "max"}})))
        .await
        .unwrap();
    assert_eq!(over.status(), StatusCode::OK);
    let none = app(state).oneshot(ask(json!({}))).await.unwrap();
    assert_eq!(none.status(), StatusCode::OK);
}

#[tokio::test]
async fn session_id_and_user_agent_reach_provider() {
    let upstream = MockServer::start().await;
    let reply = json!({
        "choices": [{"index": 0, "message": {"role": "assistant", "content": "ok"},
                     "finish_reason": "stop"}]
    });
    // Заголовок Claude Code и запасной путь через metadata.user_id дают один и тот же ID.
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header("x-opencode-session", "conv-1"))
        .and(header(
            "user-agent",
            concat!("provider-gateway-for-claude/", env!("CARGO_PKG_VERSION")),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(reply))
        .expect(2)
        .mount(&upstream)
        .await;

    let file: ProvidersFile = serde_json::from_value(json!({"providers": [{
        "id": "go", "type": "openai", "base_url": format!("{}/v1", upstream.uri()), "api_key": "k",
        "models": {"claude-go-00000000": "glm-5"}, "session_header": "X-OpenCode-Session"
    }]}))
    .unwrap();
    let state = AppState::new(
        ModelRouter::new(file).unwrap(),
        "local-token".into(),
        false,
        "user:inference".into(),
    )
    .unwrap();
    let body = json!({"model": "claude-go-00000000", "max_tokens": 32,
                      "messages": [{"role": "user", "content": "hi"}]});
    let mut by_header = post_messages(body.clone());
    by_header
        .headers_mut()
        .insert("x-claude-code-session-id", "conv-1".parse().unwrap());
    let first = app(state.clone()).oneshot(by_header).await.unwrap();
    assert_eq!(first.status(), StatusCode::OK);

    let mut with_meta = body;
    with_meta["metadata"] = json!({"user_id": r#"{"device_id":"d","session_id":"conv-1"}"#});
    let second = app(state).oneshot(post_messages(with_meta)).await.unwrap();
    assert_eq!(second.status(), StatusCode::OK);
}
