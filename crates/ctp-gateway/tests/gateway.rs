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
            "content": [{"type": "thinking", "thinking": "pondering"}],
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
    assert_eq!(body["content"][0]["thinking"], "pondering");

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
    assert!(error.contains("could not connect"), "{error}");
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

#[tokio::test]
async fn a_model_that_rejects_images_gets_the_request_without_them_and_is_remembered() {
    let upstream = MockServer::start().await;
    // Как `glm-5.3` на OpenCode: любая картинка — 400 «the request is invalid».
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(wiremock::matchers::body_string_contains("image_url"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error": {"message": "[invalid_request] the request is invalid"}
        })))
        .with_priority(1)
        .mount(&upstream)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"seen\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
            "text/event-stream",
        ))
        .with_priority(10)
        .mount(&upstream)
        .await;

    let state = state_with(&upstream.uri(), "openai", "glm-5.3");
    // Каждый запрос — с новой картинкой: одну и ту же (из истории) за подтверждение
    // не считаем, см. `the_same_picture_in_the_history_is_not_a_second_confirmation`.
    let ask = |size: u32| {
        post_messages(json!({
            "model": "claude-p-00000000", "max_tokens": 32, "stream": true,
            "messages": [{"role": "user", "content": [
                {"type": "image", "source": {"type": "base64", "media_type": "image/png",
                                             "data": screenshot_png(size, size)}},
                {"type": "text", "text": "what is this"}
            ]}]
        }))
    };
    let first = app(state.clone()).oneshot(ask(64)).await.unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let body = text_body(first).await;
    assert!(body.contains("\"text\":\"seen\""), "{body}");
    let requests = upstream.received_requests().await.unwrap();
    // Отказ на запрос с картинкой и успех без неё.
    assert_eq!(requests.len(), 2);
    let retried = String::from_utf8_lossy(&requests[1].body).to_string();
    assert!(!retried.contains("image_url"));
    assert!(retried.contains("[image omitted"));
    assert!(retried.contains("what is this"));

    // Один случай ещё не приговор: второй запрос снова пробует с картинкой (отказ и успех
    // без неё) и этим подтверждает, что дело в картинках.
    let second = app(state.clone()).oneshot(ask(65)).await.unwrap();
    assert_eq!(second.status(), StatusCode::OK);
    text_body(second).await;
    assert_eq!(upstream.received_requests().await.unwrap().len(), 4);

    // Теперь модель выучена: запрос уходит сразу без картинки, без лишнего отказа.
    let third = app(state).oneshot(ask(66)).await.unwrap();
    assert_eq!(third.status(), StatusCode::OK);
    text_body(third).await;
    assert_eq!(upstream.received_requests().await.unwrap().len(), 5);

    // Другая причина 400 (картинок нет) — ошибка как есть, без повторов.
    let plain = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error": {"message": "bad request"}
        })))
        .mount(&plain)
        .await;
    let response = app(state_with(&plain.uri(), "openai", "gpt-5.2"))
        .oneshot(post_messages(json!({
            "model": "claude-p-00000000", "max_tokens": 32, "stream": true,
            "messages": [{"role": "user", "content": "hi"}]
        })))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(plain.received_requests().await.unwrap().len(), 1);
}

const SSE_OK: &str = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";

fn refuse_when_body_has(needle: &str, message: &str) -> Mock {
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(wiremock::matchers::body_string_contains(needle))
        .respond_with(
            ResponseTemplate::new(400).set_body_json(json!({"error": {"message": message}})),
        )
        .with_priority(1)
}

fn answer_ok() -> Mock {
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(SSE_OK, "text/event-stream"))
        .with_priority(10)
}

/// Провайдер с уровнем рассуждения: в запросах есть `reasoning_effort`.
fn effort_state(base_url: &str) -> AppState {
    let file: ProvidersFile = serde_json::from_value(json!({"providers": [{
        "id": "p", "type": "openai", "base_url": base_url, "api_key": "k",
        "max_effort": "high", "default_effort": "high",
        "models": {"claude-p-00000000": "some-model"}
    }]}))
    .unwrap();
    AppState::new(
        ModelRouter::new(file).unwrap(),
        "local-token".into(),
        false,
        "user:inference".into(),
    )
    .unwrap()
}

fn plain_ask() -> Request<Body> {
    post_messages(json!({
        "model": "claude-p-00000000", "max_tokens": 32, "stream": true,
        "messages": [{"role": "user", "content": "hi"}]
    }))
}

#[tokio::test]
async fn an_unexplained_400_climbs_the_ladder_one_step_at_a_time() {
    let upstream = MockServer::start().await;
    // Провайдер не понимает `reasoning_effort` и отвечает общим 400.
    refuse_when_body_has("reasoning_effort", "the request is invalid")
        .mount(&upstream)
        .await;
    answer_ok().mount(&upstream).await;
    let state = effort_state(&upstream.uri());

    for expected_total in [2, 4] {
        let response = app(state.clone()).oneshot(plain_ask()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(text_body(response).await.contains("\"text\":\"ok\""));
        assert_eq!(
            upstream.received_requests().await.unwrap().len(),
            expected_total
        );
    }
    // Выучено: поле убирается заранее.
    let response = app(state).oneshot(plain_ask()).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    text_body(response).await;
    let requests = upstream.received_requests().await.unwrap();
    assert_eq!(requests.len(), 5);
    assert!(!String::from_utf8_lossy(&requests[4].body).contains("reasoning_effort"));
}

#[tokio::test]
async fn only_the_step_that_made_the_difference_is_learned() {
    let upstream = MockServer::start().await;
    // Мешает только `reasoning_effort`; картинки провайдер принимает.
    refuse_when_body_has("reasoning_effort", "the request is invalid")
        .mount(&upstream)
        .await;
    answer_ok().mount(&upstream).await;
    let state = effort_state(&upstream.uri());
    let with_image = || {
        post_messages(json!({
            "model": "claude-p-00000000", "max_tokens": 32, "stream": true,
            "messages": [{"role": "user", "content": [
                {"type": "image", "source": {"type": "base64", "media_type": "image/png",
                                             "data": screenshot_png(32, 32)}},
                {"type": "text", "text": "look"}
            ]}]
        }))
    };
    // Лесенка сначала убирает картинки (отказ остаётся), потом уровень рассуждения.
    for expected_total in [3, 6] {
        let response = app(state.clone()).oneshot(with_image()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        text_body(response).await;
        assert_eq!(
            upstream.received_requests().await.unwrap().len(),
            expected_total
        );
    }
    assert_eq!(
        state.profiles.snapshot(),
        vec![(
            "p/some-model".to_string(),
            vec![ctp_gateway::openai::adapt::Step::Effort]
        )]
    );
    // Дальше уходит один запрос: без уровня рассуждения, но с картинкой.
    let response = app(state).oneshot(with_image()).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    text_body(response).await;
    let requests = upstream.received_requests().await.unwrap();
    assert_eq!(requests.len(), 7);
    let last = String::from_utf8_lossy(&requests[6].body);
    assert!(last.contains("image_url"));
    assert!(!last.contains("reasoning_effort"));
}

#[tokio::test]
async fn a_422_that_echoes_the_request_still_climbs_the_ladder() {
    let upstream = MockServer::start().await;
    // FastAPI/vLLM повторяют ввод: в нём есть и «context», и «token».
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(wiremock::matchers::body_string_contains("reasoning_effort"))
        .respond_with(ResponseTemplate::new(422).set_body_json(json!({"detail": [{
            "type": "extra_forbidden", "loc": ["body", "reasoning_effort"],
            "msg": "Extra inputs are not permitted",
            "input": {"messages": [{"content": "the context and the token budget"}]}
        }]})))
        .with_priority(1)
        .mount(&upstream)
        .await;
    answer_ok().mount(&upstream).await;
    let response = app(effort_state(&upstream.uri()))
        .oneshot(plain_ask())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(text_body(response).await.contains("\"text\":\"ok\""));
    assert_eq!(upstream.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn openais_wording_about_a_long_tool_name_reaches_the_name_step_and_restores_it_without_a_stream()
 {
    let long = format!("mcp__server__{}", "w".repeat(70));
    let safe = ctp_gateway::openai::adapt::safe_name(&long);
    let upstream = MockServer::start().await;
    refuse_when_body_has(
        &"w".repeat(65),
        "Invalid 'tools[0].function.name': string too long. Expected a string with maximum length 64",
    )
    .mount(&upstream)
    .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{"index": 0, "finish_reason": "tool_calls", "message": {
                "role": "assistant", "content": null,
                "tool_calls": [{"id": "call_1", "type": "function",
                                "function": {"name": safe, "arguments": "{}"}}]}}]
        })))
        .with_priority(10)
        .mount(&upstream)
        .await;
    let response = app(state_with(&upstream.uri(), "openai", "some-model"))
        .oneshot(post_messages(json!({
            "model": "claude-p-00000000", "max_tokens": 32,
            "tools": [{"name": long, "description": "d", "input_schema": {"type": "object"}}],
            "messages": [{"role": "user", "content": "go"}]
        })))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["content"][0]["type"], "tool_use");
    assert_eq!(body["content"][0]["name"], long.as_str());
    assert_eq!(body["stop_reason"], "tool_use");
}

#[tokio::test]
async fn long_tool_names_are_shortened_for_the_provider_and_restored_for_the_client() {
    let long = format!("mcp__server__{}", "q".repeat(70));
    let safe = ctp_gateway::openai::adapt::safe_name(&long);
    let upstream = MockServer::start().await;
    // Строгий провайдер: имя инструмента длиннее 64 — 400.
    refuse_when_body_has(&"q".repeat(65), "the request is invalid")
        .mount(&upstream)
        .await;
    let call = format!(
        "data: {{\"choices\":[{{\"index\":0,\"delta\":{{\"tool_calls\":[{{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{{\"name\":\"{safe}\",\"arguments\":\"{{}}\"}}}}]}},\"finish_reason\":\"tool_calls\"}}]}}\n\ndata: [DONE]\n\n"
    );
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(call, "text/event-stream"))
        .with_priority(10)
        .mount(&upstream)
        .await;
    let response = app(state_with(&upstream.uri(), "openai", "some-model"))
        .oneshot(post_messages(json!({
            "model": "claude-p-00000000", "max_tokens": 32, "stream": true,
            "tools": [{"name": long, "description": "d", "input_schema": {"type": "object"}}],
            "messages": [{"role": "user", "content": "go"}]
        })))
        .await
        .unwrap();
    let body = text_body(response).await;
    let blocks = started_blocks(&body);
    assert_eq!(blocks[0]["type"], "tool_use", "{body}");
    assert_eq!(blocks[0]["name"], long.as_str());
    assert!(!body.contains(&safe));
}

#[tokio::test]
async fn context_overflow_becomes_anthropics_prompt_too_long_without_the_ladder() {
    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({"error": {
            "message": "This model's maximum context length is 8192 tokens, you sent 9000"
        }})))
        .mount(&upstream)
        .await;
    let response = app(effort_state(&upstream.uri()))
        .oneshot(plain_ask())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = json_body(response).await;
    assert_eq!(body["error"]["type"], "invalid_request_error");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .starts_with("prompt is too long: "),
        "{body}"
    );
    // Это не капризы модели: лесенка не гоняется, запрос один.
    assert_eq!(upstream.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_ladder_that_helps_nothing_cools_down_and_forgets_nothing() {
    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error": {"message": "the request is invalid"}
        })))
        .mount(&upstream)
        .await;
    let state = effort_state(&upstream.uri());
    let response = app(state.clone()).oneshot(plain_ask()).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    // Основной запрос и упрощённый без `reasoning_effort`; дальше шагов нет.
    assert_eq!(upstream.received_requests().await.unwrap().len(), 2);
    // Остужение: следующий отказ той же модели — без лесенки.
    let again = app(state.clone()).oneshot(plain_ask()).await.unwrap();
    assert_eq!(again.status(), StatusCode::BAD_REQUEST);
    assert_eq!(upstream.received_requests().await.unwrap().len(), 3);
    assert!(state.profiles.snapshot().is_empty());
}

#[tokio::test]
async fn provider_errors_keep_their_kind_for_claude_code() {
    for (status, kind) in [
        (401, "authentication_error"),
        (403, "permission_error"),
        (404, "not_found_error"),
        (429, "rate_limit_error"),
        (529, "overloaded_error"),
        (503, "api_error"),
    ] {
        let upstream = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(status).set_body_json(json!({
                "error": {"message": "nope"}
            })))
            .mount(&upstream)
            .await;
        let response = app(state_with(&upstream.uri(), "openai", "m"))
            .oneshot(plain_ask())
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), status);
        assert_eq!(json_body(response).await["error"]["type"], kind, "{status}");
    }
}

#[tokio::test]
async fn repeated_tool_names_are_sent_once() {
    let upstream = MockServer::start().await;
    answer_ok().mount(&upstream).await;
    let tool = json!({"name": "same", "description": "d", "input_schema": {"type": "object"}});
    let response = app(state_with(&upstream.uri(), "openai", "m"))
        .oneshot(post_messages(json!({
            "model": "claude-p-00000000", "max_tokens": 32, "stream": true,
            "tools": [tool.clone(), tool],
            "messages": [{"role": "user", "content": "hi"}]
        })))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    text_body(response).await;
    let sent: Value =
        serde_json::from_slice(&upstream.received_requests().await.unwrap()[0].body).unwrap();
    assert_eq!(sent["tools"].as_array().unwrap().len(), 1);
}

/// Base64 PNG `width`×`height` весом с настоящий скриншот (~300 КБ).
fn screenshot_png(width: u32, height: u32) -> String {
    let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
    bytes.extend(width.to_be_bytes());
    bytes.extend(height.to_be_bytes());
    bytes.extend(std::iter::repeat_n(0x5a, 300_000));
    const ABC: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().fold(0u32, |n, b| n << 8 | u32::from(*b)) << (8 * (3 - chunk.len()));
        for k in 0..=chunk.len() {
            out.push(ABC[(n >> (18 - 6 * k) & 63) as usize] as char);
        }
        out.push_str(&"=".repeat(3 - chunk.len()));
    }
    out
}

/// Скриншот так, как его присылает инструмент (glidehand): внутри `tool_result`.
fn messages_with_screenshot() -> Value {
    json!([
        {"role": "user", "content": "look"},
        {"role": "assistant", "content": [{"type": "tool_use", "id": "t1", "name": "shot", "input": {}}]},
        {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t1", "content": [
            {"type": "image", "source": {"type": "base64", "media_type": "image/png",
                                         "data": screenshot_png(1920, 1140)}},
            {"type": "text", "text": "screenshot"}
        ]}]}
    ])
}

#[tokio::test]
async fn screenshots_count_by_size_where_claude_code_reads_the_context() {
    // count_tokens (панель «Контекст») у OpenAI-провайдера — локальная оценка.
    let state = state_with("http://127.0.0.1:9", "openai", "gpt-5.2");
    let request = Request::builder()
        .method("POST")
        .uri("/v1/messages/count_tokens")
        .header("x-api-key", "local-token")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"model": "claude-p-00000000", "messages": messages_with_screenshot()})
                .to_string(),
        ))
        .unwrap();
    let counted = json_body(app(state).oneshot(request).await.unwrap()).await;
    let tokens = counted["input_tokens"].as_u64().unwrap();
    // ~1,5 тыс. за картинку, а не ~250 тыс. за её base64.
    assert!(tokens < 3000, "{tokens}");

    // input_tokens в message_start потокового ответа — по ним Claude Code решает о сжатии.
    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
            "text/event-stream",
        ))
        .mount(&upstream)
        .await;
    let response = app(state_with(&upstream.uri(), "openai", "gpt-5.2"))
        .oneshot(post_messages(json!({
            "model": "claude-p-00000000", "max_tokens": 32, "stream": true,
            "messages": messages_with_screenshot()
        })))
        .await
        .unwrap();
    let body = text_body(response).await;
    let start: Value = body
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter_map(|data| serde_json::from_str::<Value>(data).ok())
        .find(|event| event["type"] == "message_start")
        .unwrap();
    let tokens = start["message"]["usage"]["input_tokens"].as_u64().unwrap();
    assert!(tokens < 3000, "{tokens}");
}

/// Основная модель `main-model` и сильная `strong-model` — советник из настроек.
fn advisor_state(base_url: &str) -> AppState {
    let file: ProvidersFile = serde_json::from_value(json!({
        "advisor": "claude-p-11111111",
        "providers": [{
            "id": "p", "type": "openai", "base_url": base_url, "api_key": "sk-upstream",
            "models": {"claude-p-00000000": "main-model", "claude-p-11111111": "strong-model"}
        }]
    }))
    .unwrap();
    AppState::new(
        ModelRouter::new(file).unwrap(),
        "local-token".into(),
        false,
        "user:inference".into(),
    )
    .unwrap()
}

fn sse_body(chunks: &[Value]) -> String {
    let mut out: String = chunks.iter().map(|c| format!("data: {c}\n\n")).collect();
    out.push_str("data: [DONE]\n\n");
    out
}

/// Основная модель зовёт советника: ответ первой части — текст и вызов `advisor`.
async fn mount_main_calling_advisor(upstream: &MockServer) {
    let first = sse_body(&[
        json!({"choices": [{"index": 0, "delta": {"content": "Let me ask. "}, "finish_reason": null}]}),
        json!({"choices": [{"index": 0, "delta": {"tool_calls": [{"index": 0, "id": "call_adv",
            "type": "function", "function": {"name": "advisor", "arguments": "{}"}}]}, "finish_reason": null}]}),
        json!({"choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}],
               "usage": {"completion_tokens": 5}}),
    ]);
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(wiremock::matchers::body_string_contains("main-model"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(first, "text/event-stream"))
        .with_priority(10)
        .mount(upstream)
        .await;
    // Продолжение — с результатом советника в истории.
    let rest = sse_body(&[
        json!({"choices": [{"index": 0, "delta": {"content": "Done."}, "finish_reason": "stop"}],
               "usage": {"completion_tokens": 7}}),
    ]);
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(wiremock::matchers::body_string_contains("main-model"))
        .and(wiremock::matchers::body_string_contains(
            "\"tool_call_id\":\"call_adv\"",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_raw(rest, "text/event-stream"))
        .with_priority(1)
        .mount(upstream)
        .await;
}

fn ask_with_advisor() -> Request<Body> {
    post_messages(json!({
        "model": "claude-p-00000000",
        "max_tokens": 64,
        "stream": true,
        // Модель Anthropic, которой у шлюза нет: советник — из настроек.
        "tools": [{"type": "advisor_20260301", "name": "advisor", "model": "claude-fable-5-2"}],
        "messages": [{"role": "user", "content": "fix it"}]
    }))
}

/// Блоки ответа Claude Code по порядку: (тип, содержимое) из `content_block_start`.
fn started_blocks(sse: &str) -> Vec<Value> {
    sse.lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter_map(|data| serde_json::from_str::<Value>(data).ok())
        .filter(|event| event["type"] == "content_block_start")
        .map(|event| event["content_block"].clone())
        .collect()
}

#[tokio::test]
async fn gateway_answers_the_advisor_and_continues_the_same_reply() {
    let upstream = MockServer::start().await;
    mount_main_calling_advisor(&upstream).await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(wiremock::matchers::body_string_contains("strong-model"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "ADVICE: test first"},
                         "finish_reason": "stop"}]
        })))
        .expect(1)
        .mount(&upstream)
        .await;

    let response = app(advisor_state(&upstream.uri()))
        .oneshot(ask_with_advisor())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = text_body(response).await;
    let blocks = started_blocks(&body);
    let kinds: Vec<&str> = blocks.iter().filter_map(|b| b["type"].as_str()).collect();
    assert_eq!(
        kinds,
        ["text", "server_tool_use", "advisor_tool_result", "text"],
        "{body}"
    );
    assert_eq!(blocks[1]["name"], "advisor");
    // ID вернётся в истории как tool_call_id: у OpenAI предел — 40 символов.
    assert!(blocks[1]["id"].as_str().unwrap().len() <= 40);
    assert_eq!(blocks[2]["tool_use_id"], blocks[1]["id"]);
    assert_eq!(blocks[2]["content"]["type"], "advisor_result");
    assert_eq!(blocks[2]["content"]["text"], "ADVICE: test first");
    assert!(body.contains("\"text\":\"Done.\""));
    assert!(body.contains("\"stop_reason\":\"end_turn\""));
    // Токены обоих отрезков.
    assert!(body.contains("\"output_tokens\":12"), "{body}");
    assert!(!body.contains("\"type\":\"tool_use\""));
    assert_eq!(body.matches("event: message_start").count(), 1);

    // Модели объявлена функция `advisor`; советник получил разговор текстом, без tools.
    let requests = upstream.received_requests().await.unwrap();
    let bodies: Vec<Value> = requests
        .iter()
        .map(|r| serde_json::from_slice(&r.body).unwrap())
        .collect();
    let first = bodies.iter().find(|b| b["model"] == "main-model").unwrap();
    assert!(
        first["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["function"]["name"] == "advisor")
    );
    let advice = bodies
        .iter()
        .find(|b| b["model"] == "strong-model")
        .unwrap();
    assert!(advice.get("tools").is_none());
    // Потоком: иначе на большом разговоре посредники рвут молчащее соединение.
    assert_eq!(advice["stream"], true);
    let transcript = advice["messages"][1]["content"].as_str().unwrap();
    assert!(transcript.contains("fix it") && transcript.contains("Let me ask."));
    // В продолжении: ответ ассистента с вызовом и результат советника.
    let rest = bodies
        .iter()
        .find(|b| b.to_string().contains("tool_call_id"))
        .unwrap();
    let messages = rest["messages"].as_array().unwrap();
    let n = messages.len();
    assert_eq!(
        messages[n - 2]["tool_calls"][0]["function"]["name"],
        "advisor"
    );
    assert_eq!(messages[n - 2]["content"], "Let me ask. ");
    assert_eq!(messages[n - 1]["content"], "ADVICE: test first");
}

#[tokio::test]
async fn advisor_calls_are_capped_even_if_the_model_keeps_calling() {
    let upstream = MockServer::start().await;
    // Модель зовёт советника всегда, даже после того, как функцию у неё убрали; и
    // дважды в одном ответе — второй вызов не должен уйти в Claude Code.
    let calling = sse_body(&[
        json!({"choices": [{"index": 0, "delta": {"tool_calls": [
            {"index": 0, "id": "call_a", "type": "function", "function": {"name": "advisor", "arguments": "{}"}},
            {"index": 1, "id": "call_b", "type": "function", "function": {"name": "advisor", "arguments": "{}"}}
        ]}, "finish_reason": null}]}),
        json!({"choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}]}),
    ]);
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(wiremock::matchers::body_string_contains("main-model"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(calling, "text/event-stream"))
        .mount(&upstream)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(wiremock::matchers::body_string_contains("strong-model"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "advice"},
                         "finish_reason": "stop"}]
        })))
        .expect(3)
        .mount(&upstream)
        .await;

    // Без других инструментов: после лимита `tools` пустеет и уходит из запроса целиком.
    let response = app(advisor_state(&upstream.uri()))
        .oneshot(ask_with_advisor())
        .await
        .unwrap();
    let body = text_body(response).await;
    let results: Vec<Value> = started_blocks(&body)
        .into_iter()
        .filter(|b| b["type"] == "advisor_tool_result")
        .collect();
    assert_eq!(results.len(), 4, "{body}");
    assert_eq!(results[3]["content"]["error_code"], "max_uses_exceeded");
    assert!(!body.contains("\"type\":\"tool_use\""), "{body}");
    // Вызовов для Claude Code нет — и стоп не «жду инструмент».
    assert!(body.contains("\"stop_reason\":\"end_turn\""), "{body}");
    assert!(body.contains("event: message_stop"));

    let requests = upstream.received_requests().await.unwrap();
    let last_main: Value = requests
        .iter()
        .map(|r| serde_json::from_slice::<Value>(&r.body).unwrap())
        .rfind(|b| b["model"] == "main-model")
        .unwrap();
    assert!(last_main.get("tools").is_none(), "{last_main}");
    assert!(last_main.get("tool_choice").is_none());
}

#[tokio::test]
async fn streamed_advice_is_collected_without_the_reasoning() {
    let upstream = MockServer::start().await;
    mount_main_calling_advisor(&upstream).await;
    let advice = sse_body(&[
        json!({"choices": [{"index": 0, "delta": {"reasoning_content": "thinking…"}, "finish_reason": null}]}),
        json!({"choices": [{"index": 0, "delta": {"content": "Test "}, "finish_reason": null}]}),
        json!({"choices": [{"index": 0, "delta": {"content": "first."}, "finish_reason": "stop"}]}),
    ]);
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(wiremock::matchers::body_string_contains("strong-model"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(advice, "text/event-stream"))
        .mount(&upstream)
        .await;

    let response = app(advisor_state(&upstream.uri()))
        .oneshot(ask_with_advisor())
        .await
        .unwrap();
    let body = text_body(response).await;
    let blocks = started_blocks(&body);
    assert_eq!(blocks[2]["content"]["type"], "advisor_result", "{body}");
    assert_eq!(blocks[2]["content"]["text"], "Test first.");
    assert!(body.contains("\"text\":\"Done.\""));
}

#[tokio::test]
async fn advice_cut_off_mid_stream_is_not_passed_as_whole() {
    let upstream = MockServer::start().await;
    mount_main_calling_advisor(&upstream).await;
    // Ни `[DONE]`, ни `finish_reason`: провайдер закрыл поток на полуслове.
    let cut = format!(
        "data: {}\n\n",
        json!({"choices": [{"index": 0, "delta": {"content": "Test fir"}, "finish_reason": null}]})
    );
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(wiremock::matchers::body_string_contains("strong-model"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(cut, "text/event-stream"))
        .mount(&upstream)
        .await;

    let response = app(advisor_state(&upstream.uri()))
        .oneshot(ask_with_advisor())
        .await
        .unwrap();
    let body = text_body(response).await;
    let blocks = started_blocks(&body);
    assert_eq!(
        blocks[2]["content"]["type"], "advisor_tool_result_error",
        "{body}"
    );
    assert_eq!(blocks[2]["content"]["error_code"], "unavailable");
    assert!(body.contains("\"text\":\"Done.\""));
}

#[tokio::test]
async fn advisor_failure_is_a_result_and_the_reply_goes_on() {
    let upstream = MockServer::start().await;
    mount_main_calling_advisor(&upstream).await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(wiremock::matchers::body_string_contains("strong-model"))
        .respond_with(ResponseTemplate::new(429).set_body_json(json!({
            "error": {"message": "slow down sk-upstream"}
        })))
        .mount(&upstream)
        .await;

    let response = app(advisor_state(&upstream.uri()))
        .oneshot(ask_with_advisor())
        .await
        .unwrap();
    let body = text_body(response).await;
    let blocks = started_blocks(&body);
    assert_eq!(blocks[2]["content"]["type"], "advisor_tool_result_error");
    assert_eq!(blocks[2]["content"]["error_code"], "too_many_requests");
    assert!(body.contains("\"text\":\"Done.\""));
    assert!(!body.contains("sk-upstream"));
}

#[tokio::test]
async fn without_an_advisor_model_the_tool_is_not_offered() {
    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            sse_body(&[json!({"choices": [{"index": 0, "delta": {"content": "ok"}, "finish_reason": "stop"}]})]),
            "text/event-stream",
        ))
        .mount(&upstream)
        .await;
    // Нет `advisor` в настройках, а Claude Code назвал модель, которой у шлюза нет.
    let response = app(state_with(&upstream.uri(), "openai", "gpt-5.2"))
        .oneshot(ask_with_advisor_for("claude-p-00000000"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let sent: Value =
        serde_json::from_slice(&upstream.received_requests().await.unwrap()[0].body).unwrap();
    assert!(sent.get("tools").is_none(), "{sent}");
}

fn ask_with_advisor_for(model: &str) -> Request<Body> {
    post_messages(json!({
        "model": model,
        "max_tokens": 64,
        "stream": true,
        "tools": [{"type": "advisor_20260301", "name": "advisor", "model": "claude-fable-5-2"}],
        "messages": [{"role": "user", "content": "fix it"}]
    }))
}

#[tokio::test]
async fn anthropic_provider_gets_no_advisor_tool_or_blocks() {
    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "msg_x", "type": "message", "role": "assistant", "model": "m",
            "content": [{"type": "text", "text": "ok"}], "stop_reason": "end_turn",
            "stop_sequence": null, "usage": {"input_tokens": 1, "output_tokens": 1}
        })))
        .mount(&upstream)
        .await;
    let response = app(state_with(
        &upstream.uri(),
        "anthropic",
        "claude-sonnet-4-5",
    ))
    .oneshot(post_messages(json!({
        "model": "claude-p-00000000",
        "max_tokens": 32,
        "tools": [
            {"type": "advisor_20260301", "name": "advisor", "model": "claude-fable-5-2"},
            {"name": "Read", "input_schema": {"type": "object"}}
        ],
        "messages": [
            {"role": "user", "content": "hi"},
            {"role": "assistant", "content": [
                {"type": "text", "text": "asking"},
                {"type": "server_tool_use", "id": "srvtoolu_1", "name": "advisor", "input": {}},
                {"type": "advisor_tool_result", "tool_use_id": "srvtoolu_1",
                 "content": {"type": "advisor_result", "text": "advice"}}
            ]},
            {"role": "user", "content": "go on"}
        ]
    })))
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let sent: Value =
        serde_json::from_slice(&upstream.received_requests().await.unwrap()[0].body).unwrap();
    assert_eq!(sent["tools"].as_array().unwrap().len(), 1);
    assert_eq!(sent["tools"][0]["name"], "Read");
    assert_eq!(
        sent["messages"][1]["content"],
        json!([{"type": "text", "text": "asking"}])
    );
}

#[tokio::test]
async fn the_same_picture_in_the_history_is_not_a_second_confirmation() {
    let upstream = MockServer::start().await;
    // Битая картинка: провайдер отказывает на ней в любом ходе, где она есть в истории.
    refuse_when_body_has("image_url", "You uploaded an unsupported image")
        .mount(&upstream)
        .await;
    answer_ok().mount(&upstream).await;
    let state = state_with(&upstream.uri(), "openai", "gpt-x");
    let turn = |text: &str| {
        post_messages(json!({
            "model": "claude-p-00000000", "max_tokens": 32, "stream": true,
            "messages": [
                {"role": "user", "content": [
                    {"type": "image", "source": {"type": "base64", "media_type": "image/png",
                                                 "data": screenshot_png(40, 40)}},
                    {"type": "text", "text": "look"}]},
                {"role": "assistant", "content": "ok"},
                {"role": "user", "content": text}
            ]
        }))
    };
    for text in ["one", "two", "three"] {
        let response = app(state.clone()).oneshot(turn(text)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        text_body(response).await;
    }
    assert!(state.profiles.snapshot().is_empty());
}

#[tokio::test]
async fn an_echoing_422_does_not_cut_max_tokens_to_a_number_from_the_echo() {
    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(wiremock::matchers::body_string_contains("reasoning_effort"))
        .respond_with(ResponseTemplate::new(422).set_body_json(json!({"detail": [{
            "type": "extra_forbidden", "loc": ["body", "reasoning_effort"],
            "msg": "Extra inputs are not permitted",
            "input": {"max_tokens": 32000, "note": "line 120 of the context"}
        }]})))
        .with_priority(1)
        .mount(&upstream)
        .await;
    answer_ok().mount(&upstream).await;
    let response = app(effort_state(&upstream.uri()))
        .oneshot(post_messages(json!({
            "model": "claude-p-00000000", "max_tokens": 32000, "stream": true,
            "messages": [{"role": "user", "content": "hi"}]
        })))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    text_body(response).await;
    let requests = upstream.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    let retried: Value = serde_json::from_slice(&requests[1].body).unwrap();
    assert!(retried.get("reasoning_effort").is_none());
    let sent = retried
        .get("max_tokens")
        .or_else(|| retried.get("max_completion_tokens"))
        .and_then(Value::as_u64);
    assert_eq!(sent, Some(32000));
}

#[tokio::test]
async fn an_overflow_error_inside_the_stream_reaches_claude_code_as_prompt_too_long() {
    let upstream = MockServer::start().await;
    let sse = r#"data: {"error":{"message":"This model's maximum context length is 8192 tokens. However, your messages resulted in 9000 tokens."}}

"#;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
        .mount(&upstream)
        .await;
    let response = app(state_with(&upstream.uri(), "openai", "m"))
        .oneshot(plain_ask())
        .await
        .unwrap();
    let body = text_body(response).await;
    assert!(body.contains("invalid_request_error"), "{body}");
    assert!(
        body.contains("prompt is too long: 9000 tokens > 8192 maximum"),
        "{body}"
    );
}
