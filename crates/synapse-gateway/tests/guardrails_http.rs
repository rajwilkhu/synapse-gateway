//! End-to-end: a route with a blocking policy returns HTTP 400 with the
//! OpenAI-shaped content-policy error before any upstream call is made.

#![cfg(feature = "server")]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt; // for `oneshot`

use synapse::gateway::{Gateway, GatewayBuilder};
use synapse::guard::{GuardEngine, GuardrailsConfig};
use synapse::ledger::{InMemoryLedger, LedgerHandle, LedgerStore};
use synapse::pricing::PricingTable;
use synapse::providers::Catalog;
use synapse::routing::table::RouteTable;
use synapse::server::router;
use synapse::telemetry::{scrape, test_metrics};
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn blocked_gateway() -> Gateway {
    let routes = RouteTable::from_toml_str(
        r#"[routes."fast"]
           policy = "strict"
           legs = [{ provider = "qwen", model = "qwen-max" }]"#,
    )
    .unwrap();
    let guard = GuardEngine::from_config(
        &GuardrailsConfig::from_toml_str(
            r#"[guardrails.strict]
               scanners = [{ type = "ban_substrings", substrings = ["forbidden"] }]"#,
        )
        .unwrap(),
    )
    .unwrap();
    // Build a catalog pointing at an unreachable upstream — the guardrail fires
    // before any network call, so this URL is never actually contacted.
    let catalog = Catalog::build(
        &HashMap::from([
            ("DASHSCOPE_API_KEY".to_string(), "sk-test".to_string()),
            (
                "DASHSCOPE_BASE_URL".to_string(),
                "http://127.0.0.1:1/v1".to_string(),
            ),
        ]),
        &routes.referenced_providers(),
        Duration::from_secs(5),
    )
    .unwrap();
    let ledger = LedgerHandle::spawn(
        Arc::new(InMemoryLedger::default()) as Arc<dyn LedgerStore>,
        16,
    );
    Gateway::builder()
        .routes(routes)
        .catalog(catalog)
        .pricing(PricingTable::default())
        .ledger(ledger)
        .guard(guard)
        .build()
        .unwrap()
}

#[tokio::test]
async fn blocked_request_returns_400_content_policy_violation() {
    let app = router(Arc::new(blocked_gateway()));
    let body = serde_json::json!({
        "model": "fast",
        "messages": [{ "role": "user", "content": "this is forbidden" }]
    })
    .to_string();
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    // Security invariant: the matched input text must never be echoed back.
    let body_str = String::from_utf8_lossy(&bytes);
    assert!(
        !body_str.contains("forbidden"),
        "block response must not leak the matched input text: {body_str}"
    );
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["error"]["type"], "content_policy_violation");
    assert_eq!(json["error"]["code"], "content_blocked");
    assert_eq!(json["error"]["scanners"][0], "ban_substrings");
    assert_eq!(json["error"]["patterns"], serde_json::json!([]));
}

#[tokio::test]
async fn blocked_input_request_writes_blocked_guard_ledger_row() {
    let store = Arc::new(InMemoryLedger::default());
    let gateway = builder_with(
        "http://127.0.0.1:1",
        r#"[guardrails.strict]
           scanners = [{ type = "ban_substrings", substrings = ["forbidden"] }]"#,
    )
    .ledger(LedgerHandle::spawn(
        store.clone() as Arc<dyn LedgerStore>,
        16,
    ))
    .build()
    .unwrap();
    let app = router(Arc::new(gateway));
    let resp = app
        .oneshot(chat_request("fast", "this is forbidden"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    for _ in 0..100 {
        if !store.entries.lock().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let entries = store.entries.lock().clone();
    assert_eq!(entries.len(), 1, "{entries:?}");
    let row = &entries[0];
    assert_eq!(row.lane, "guard");
    assert_eq!(row.status, "blocked");
    assert_eq!(row.op, "guard_input");
    assert_eq!(row.provider, "synapse");
    assert_eq!(row.model, "strict");
    assert_eq!(row.route, "fast");
    assert_eq!(row.input_tokens, 0);
    assert_eq!(row.output_tokens, 0);
}

const SECRET: &str = "ghp_aBcDeFgHiJkLmNoPqRsTuVwXyZ0123456789";

/// The chat lane streams from the provider even for non-streaming requests.
fn sse_ok(content: &str) -> String {
    format!(
        "data: {{\"choices\":[{{\"delta\":{{\"content\":\"{content}\"}}}}]}}\n\n\
         data: {{\"choices\":[{{\"delta\":{{}},\"finish_reason\":\"stop\"}}],\"usage\":{{\"prompt_tokens\":3,\"completion_tokens\":5,\"total_tokens\":8}}}}\n\n\
         data: [DONE]\n\n"
    )
}

const MASK_SECRETS: &str = r#"[guardrails.strict]
    scanners = [{ type = "secrets", action = "mask" }]"#;

fn gateway_with(upstream: &str, guardrails: &str) -> Gateway {
    builder_with(upstream, guardrails).build().unwrap()
}

fn builder_with(upstream: &str, guardrails: &str) -> GatewayBuilder {
    let routes = RouteTable::from_toml_str(
        r#"[routes."fast"]
           policy = "strict"
           legs = [{ provider = "qwen", model = "qwen-max" }]"#,
    )
    .unwrap();
    let catalog = Catalog::build(
        &HashMap::from([
            ("DASHSCOPE_API_KEY".to_string(), "sk-test".to_string()),
            ("DASHSCOPE_BASE_URL".to_string(), format!("{upstream}/v1/")),
        ]),
        &routes.referenced_providers(),
        Duration::from_secs(5),
    )
    .unwrap();
    let ledger = LedgerHandle::spawn(
        Arc::new(InMemoryLedger::default()) as Arc<dyn LedgerStore>,
        16,
    );
    Gateway::builder()
        .routes(routes)
        .catalog(catalog)
        .pricing(PricingTable::default())
        .ledger(ledger)
        .guard(
            GuardEngine::from_config(&GuardrailsConfig::from_toml_str(guardrails).unwrap())
                .unwrap(),
        )
}

fn chat_request(model: &str, content: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({"model": model, "messages": [{"role": "user", "content": content}]})
                .to_string(),
        ))
        .unwrap()
}

#[tokio::test]
async fn masked_chat_request_reaches_the_provider_redacted_and_is_signalled() {
    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains("[REDACTED:secrets]"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(sse_ok("ok"), "text/event-stream"))
        .expect(1)
        .mount(&upstream)
        .await;
    let app = router(Arc::new(gateway_with(&upstream.uri(), MASK_SECRETS)));
    let resp = app
        .oneshot(chat_request("fast", &format!("my token is {SECRET}")))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()["x-synapse-guard"], "masked");
    let seen = &upstream.received_requests().await.unwrap()[0];
    assert!(!String::from_utf8_lossy(&seen.body).contains(SECRET));
}

#[tokio::test]
async fn masked_stream_request_carries_the_header() {
    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains("[REDACTED:secrets]"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(sse_ok("ok"), "text/event-stream"))
        .expect(1)
        .mount(&upstream)
        .await;
    let app = router(Arc::new(gateway_with(&upstream.uri(), MASK_SECRETS)));
    let mut req = chat_request("fast", "unused");
    *req.body_mut() = Body::from(
        serde_json::json!({"model": "fast", "stream": true,
            "messages": [{"role": "user", "content": format!("my token is {SECRET}")}]})
        .to_string(),
    );
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()["x-synapse-guard"], "masked");
    let _ = resp.into_body().collect().await.unwrap();
    let seen = &upstream.received_requests().await.unwrap()[0];
    assert!(!String::from_utf8_lossy(&seen.body).contains(SECRET));
}

#[tokio::test]
async fn masked_request_records_one_masked_scan() {
    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(sse_ok("ok"), "text/event-stream"))
        .mount(&upstream)
        .await;
    let (metrics, exporter) = test_metrics();
    let gateway = builder_with(&upstream.uri(), MASK_SECRETS)
        .metrics(metrics)
        .build()
        .unwrap();
    let resp = router(Arc::new(gateway))
        .oneshot(chat_request("fast", &format!("my token is {SECRET}")))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let line = r#"synapse_guard_scans_total{direction="input",lane="chat",outcome="masked",policy="strict"} 1"#;
    let text = scrape(&exporter);
    assert!(
        text.lines().any(|l| l == line),
        "missing `{line}` in:\n{text}"
    );
}

#[tokio::test]
async fn unmasked_request_has_no_guard_header() {
    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(sse_ok("ok"), "text/event-stream"))
        .mount(&upstream)
        .await;
    let app = router(Arc::new(gateway_with(&upstream.uri(), MASK_SECRETS)));
    let resp = app
        .oneshot(chat_request("fast", "nothing secret here"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(resp.headers().get("x-synapse-guard").is_none());
}

#[tokio::test]
async fn block_and_mask_in_one_request_is_400_with_no_upstream_call() {
    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&upstream)
        .await;
    let app = router(Arc::new(gateway_with(
        &upstream.uri(),
        r#"[guardrails.strict]
           scanners = [{ type = "secrets", action = "mask" },
                       { type = "ban_substrings", substrings = ["forbidden"] }]"#,
    )));
    let resp = app
        .oneshot(chat_request("fast", &format!("forbidden {SECRET}")))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn observe_mode_forwards_the_original_text() {
    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_string_contains(SECRET))
        .respond_with(ResponseTemplate::new(200).set_body_raw(sse_ok("ok"), "text/event-stream"))
        .expect(1)
        .mount(&upstream)
        .await;
    let app = router(Arc::new(gateway_with(
        &upstream.uri(),
        r#"[guardrails.strict]
           mode = "observe"
           scanners = [{ type = "secrets", action = "mask" }]"#,
    )));
    let resp = app
        .oneshot(chat_request("fast", &format!("my token is {SECRET}")))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(resp.headers().get("x-synapse-guard").is_none());
}

const OUTPUT_SCANNERS: &str = r#"[guardrails.strict]
    scanners = []
    output_scanners = [{ type = "secrets", action = "mask" },
                       { type = "ban_substrings", substrings = ["forbidden"] }]"#;

async fn answer_with(upstream: &MockServer, content: &str) {
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(sse_ok(content), "text/event-stream"))
        .mount(upstream)
        .await;
}

async fn body_bytes(resp: axum::response::Response) -> Vec<u8> {
    resp.into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec()
}

#[tokio::test]
async fn masked_response_content_and_header() {
    let upstream = MockServer::start().await;
    answer_with(&upstream, &format!("here: {SECRET}")).await;
    let app = router(Arc::new(gateway_with(&upstream.uri(), OUTPUT_SCANNERS)));
    let resp = app.oneshot(chat_request("fast", "hi")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()["x-synapse-guard"], "masked");
    let json: serde_json::Value = serde_json::from_slice(&body_bytes(resp).await).unwrap();
    assert_eq!(
        json["choices"][0]["message"]["content"],
        "here: [REDACTED:secrets]"
    );
}

#[tokio::test]
async fn blocked_response_drops_tool_calls() {
    let upstream = MockServer::start().await;
    let sse = "data: {\"choices\":[{\"delta\":{\"content\":\"forbidden words\"}}]}\n\n\
               data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\
               \"function\":{\"name\":\"run\",\"arguments\":\"{}\"}}]}}]}\n\n\
               data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n\
               data: [DONE]\n\n";
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
        .mount(&upstream)
        .await;
    let app = router(Arc::new(gateway_with(&upstream.uri(), OUTPUT_SCANNERS)));
    let resp = app.oneshot(chat_request("fast", "hi")).await.unwrap();
    assert_eq!(resp.headers()["x-synapse-guard"], "blocked");
    let json: serde_json::Value = serde_json::from_slice(&body_bytes(resp).await).unwrap();
    let message = &json["choices"][0]["message"];
    assert!(
        message
            .get("tool_calls")
            .is_none_or(|t| t.as_array().is_some_and(|a| a.is_empty())),
        "{json}"
    );
}

#[tokio::test]
async fn blocked_response_is_content_filter() {
    let upstream = MockServer::start().await;
    answer_with(&upstream, "forbidden words").await;
    let store = Arc::new(InMemoryLedger::default());
    let gateway = builder_with(&upstream.uri(), OUTPUT_SCANNERS)
        .ledger(LedgerHandle::spawn(
            store.clone() as Arc<dyn LedgerStore>,
            16,
        ))
        .build()
        .unwrap();
    let resp = router(Arc::new(gateway))
        .oneshot(chat_request("fast", "hi"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()["x-synapse-guard"], "blocked");
    let json: serde_json::Value = serde_json::from_slice(&body_bytes(resp).await).unwrap();
    assert_eq!(json["choices"][0]["message"]["content"], "");
    assert_eq!(json["choices"][0]["finish_reason"], "content_filter");
    for _ in 0..100 {
        if !store.entries.lock().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(store.entries.lock()[0].status, "blocked");
}

#[tokio::test]
async fn no_output_scanners_leaves_the_response_byte_for_byte() {
    let upstream = MockServer::start().await;
    answer_with(&upstream, &format!("here: {SECRET}")).await;
    let guarded = router(Arc::new(gateway_with(&upstream.uri(), MASK_SECRETS)));
    let plain = router(Arc::new(builder_with(&upstream.uri(), "").build().unwrap()));
    let a = guarded.oneshot(chat_request("fast", "hi")).await.unwrap();
    let b = plain.oneshot(chat_request("fast", "hi")).await.unwrap();
    assert!(a.headers().get("x-synapse-guard").is_none());
    let (a, b) = (body_bytes(a).await, body_bytes(b).await);
    let strip = |v: &[u8]| {
        let mut j: serde_json::Value = serde_json::from_slice(v).unwrap();
        j.as_object_mut()
            .unwrap()
            .retain(|k, _| k != "id" && k != "created");
        j
    };
    assert_eq!(strip(&a), strip(&b));
}

fn sse_chunks(parts: &[&str]) -> String {
    let mut out = String::new();
    for p in parts {
        out += &format!(
            "data: {}\n\n",
            serde_json::json!({"id": "x", "object": "chat.completion.chunk",
                "choices": [{"index": 0, "delta": {"content": p}, "finish_reason": null}]})
        );
    }
    out += &format!(
        "data: {}\n\n",
        serde_json::json!({"id": "x", "object": "chat.completion.chunk",
            "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 3, "completion_tokens": 4, "total_tokens": 7}})
    );
    out + "data: [DONE]\n\n"
}

async fn stream_with(upstream: &MockServer, parts: &[&str]) {
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(sse_chunks(parts), "text/event-stream"),
        )
        .mount(upstream)
        .await;
}

fn stream_request(model: &str) -> Request<Body> {
    let mut req = chat_request(model, "unused");
    *req.body_mut() = Body::from(
        serde_json::json!({"model": model, "stream": true,
            "messages": [{"role": "user", "content": "hi"}]})
        .to_string(),
    );
    req
}

/// The `data:` payloads of an SSE body, in order.
fn sse_data(body: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(body)
        .split("\n\n")
        .filter_map(|e| e.strip_prefix("data: ").map(str::to_string))
        .collect()
}

/// Concatenated `delta.content` of every chunk.
fn streamed_content(events: &[String]) -> String {
    events
        .iter()
        .filter_map(|e| serde_json::from_str::<serde_json::Value>(e).ok())
        .filter_map(|j| {
            j["choices"][0]["delta"]["content"]
                .as_str()
                .map(str::to_string)
        })
        .collect()
}

async fn first_ledger_status(store: &InMemoryLedger) -> String {
    for _ in 0..100 {
        if !store.entries.lock().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    store.entries.lock()[0].status.clone()
}

#[tokio::test]
async fn stream_masks_a_secret_split_across_chunks() {
    let upstream = MockServer::start().await;
    stream_with(&upstream, &["key: ", &SECRET[..10], &SECRET[10..], " end"]).await;
    let app = router(Arc::new(gateway_with(&upstream.uri(), OUTPUT_SCANNERS)));
    let resp = app.oneshot(stream_request("fast")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let events = sse_data(&body_bytes(resp).await);
    assert_eq!(streamed_content(&events), "key: [REDACTED:secrets] end");
    let done: serde_json::Value = serde_json::from_str(&events[events.len() - 2]).unwrap();
    assert_eq!(done["choices"][0]["finish_reason"], "stop");
    assert_eq!(events.last().unwrap(), "[DONE]");
}

/// The window fits the shortest GitHub token (40 chars); the token is longer,
/// so it is masked whole only because a match crossing the release point is
/// held from its start.
#[tokio::test]
async fn stream_masks_a_long_secret_split_across_many_chunks_whole() {
    let long = format!("ghp_{}", "aB3dE6gH9j".repeat(4));
    let upstream = MockServer::start().await;
    let parts: Vec<&str> = vec![
        "token ",
        &long[..6],
        &long[6..17],
        &long[17..29],
        &long[29..],
        " done",
    ];
    stream_with(&upstream, &parts).await;
    let app = router(Arc::new(gateway_with(
        &upstream.uri(),
        &format!("{OUTPUT_SCANNERS}\n    output_holdback_chars = 40"),
    )));
    let resp = app.oneshot(stream_request("fast")).await.unwrap();
    let events = sse_data(&body_bytes(resp).await);
    assert_eq!(streamed_content(&events), "token [REDACTED:secrets] done");
}

#[tokio::test]
async fn stream_masks_a_token_whose_chunks_end_at_hyphens_whole() {
    // Split so secret scanners do not flag the fixture.
    let token = concat!(
        "xo",
        "xb-123456789012-1234567890123-AbCdEfGhIjKlMnOpQrStUvWx"
    );
    let lead = "x ".repeat(60);
    let mut parts = vec![lead.as_str(), "token "];
    parts.extend(token.split_inclusive('-'));
    parts.push(" end");
    let upstream = MockServer::start().await;
    stream_with(&upstream, &parts).await;
    let app = router(Arc::new(gateway_with(
        &upstream.uri(),
        &format!("{OUTPUT_SCANNERS}\n    output_holdback_chars = 64"),
    )));
    let resp = app.oneshot(stream_request("fast")).await.unwrap();
    let events = sse_data(&body_bytes(resp).await);
    assert_eq!(
        streamed_content(&events),
        format!("{lead}token [REDACTED:secrets] end")
    );
}

#[tokio::test]
async fn stream_block_ends_with_content_filter_and_error_event() {
    let upstream = MockServer::start().await;
    stream_with(&upstream, &["fine ", "forbidden", " more"]).await;
    let store = Arc::new(InMemoryLedger::default());
    let gateway = builder_with(
        &upstream.uri(),
        &format!("{OUTPUT_SCANNERS}\n    output_holdback_chars = 64"),
    )
    .ledger(LedgerHandle::spawn(
        store.clone() as Arc<dyn LedgerStore>,
        16,
    ))
    .build()
    .unwrap();
    let resp = router(Arc::new(gateway))
        .oneshot(stream_request("fast"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let events = sse_data(&body_bytes(resp).await);
    assert!(!streamed_content(&events).contains("forbidden"));
    let n = events.len();
    assert!(n >= 3, "{events:?}");
    let done: serde_json::Value = serde_json::from_str(&events[n - 3]).unwrap();
    assert_eq!(done["choices"][0]["finish_reason"], "content_filter");
    let err: serde_json::Value = serde_json::from_str(&events[n - 2]).unwrap();
    assert_eq!(err["error"]["code"], "content_blocked");
    assert_eq!(err["error"]["type"], "content_policy_violation");
    assert_eq!(err["error"]["scanners"][0], "ban_substrings");
    assert!(!events[n - 2].contains("forbidden"));
    assert_eq!(events[n - 1], "[DONE]");
    assert_eq!(first_ledger_status(&store).await, "blocked");
}

#[tokio::test]
async fn stream_without_output_scanners_is_unchanged() {
    let upstream = MockServer::start().await;
    stream_with(&upstream, &["key: ", &SECRET[..10], &SECRET[10..], " end"]).await;
    let guarded = router(Arc::new(gateway_with(&upstream.uri(), MASK_SECRETS)));
    let plain = router(Arc::new(builder_with(&upstream.uri(), "").build().unwrap()));
    let a = guarded.oneshot(stream_request("fast")).await.unwrap();
    let b = plain.oneshot(stream_request("fast")).await.unwrap();
    let strip = |body: Vec<u8>| -> Vec<String> {
        sse_data(&body)
            .into_iter()
            .map(|e| match serde_json::from_str::<serde_json::Value>(&e) {
                Ok(mut j) => {
                    j.as_object_mut().unwrap().remove("id");
                    j.to_string()
                }
                Err(_) => e,
            })
            .collect()
    };
    let (a, b) = (strip(body_bytes(a).await), strip(body_bytes(b).await));
    assert_eq!(a, b);
    assert!(streamed_content(&a).contains(SECRET));
}

#[tokio::test]
async fn stream_dropped_mid_reply_records_one_scan_and_the_ledger_row() {
    let upstream = MockServer::start().await;
    stream_with(&upstream, &["aaaaaaaa", "bbbbbbbb", "cccccccc"]).await;
    let store = Arc::new(InMemoryLedger::default());
    let (metrics, exporter) = test_metrics();
    let gateway = builder_with(
        &upstream.uri(),
        &format!("{OUTPUT_SCANNERS}\n    output_holdback_chars = 4"),
    )
    .metrics(metrics)
    .ledger(LedgerHandle::spawn(
        store.clone() as Arc<dyn LedgerStore>,
        16,
    ))
    .build()
    .unwrap();
    let resp = router(Arc::new(gateway))
        .oneshot(stream_request("fast"))
        .await
        .unwrap();
    let mut body = resp.into_body();
    let first = body.frame().await.unwrap().unwrap().into_data().unwrap();
    assert_eq!(streamed_content(&sse_data(&first)), "aaaa");
    drop(body);
    assert_eq!(first_ledger_status(&store).await, "ok");
    let line = r#"synapse_guard_scans_total{direction="output",lane="chat",outcome="pass",policy="strict"} 1"#;
    let text = scrape(&exporter);
    assert!(
        text.lines().any(|l| l == line),
        "missing `{line}` in:\n{text}"
    );
}
