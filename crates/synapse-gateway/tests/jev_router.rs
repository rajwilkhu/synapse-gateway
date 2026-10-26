#![cfg(feature = "server")]

//! Jev router: `strategy = "jev"` routes serve each request from the tier Jev
//! rates it at, with that tier's reasoning effort, failing open to
//! `default_tier`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use synapse::gateway::Gateway;
use synapse::guard::{GuardEngine, GuardrailsConfig};
use synapse::jev_native::JevNativeProvider;
use synapse::ledger::{InMemoryLedger, LedgerHandle, LedgerStore, UsageEntry};
use synapse::pricing::PricingTable;
use synapse::providers::vertex_auth::VertexAuth;
use synapse::providers::Catalog;
use synapse::routing::table::RouteTable;
use synapse::server::router;
use synapse::telemetry::{scrape, test_metrics};
use synapse::vertex_native::VertexNativeProvider;
use tower::ServiceExt;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const ROUTES: &str = r#"
[routes."auto"]
strategy = "jev"

[routes."auto".jev_router]
default_tier = "moderate"
timeout_ms = 2000

[[routes."auto".tiers]]
name = "trivial"
description = "Greetings and one-line lookups"
effort = "none"
legs = [{ provider = "qwen", model = "qwen-flash" }]

[[routes."auto".tiers]]
name = "moderate"
description = "Everyday questions and simple edits"
effort = "low"
legs = [{ provider = "qwen", model = "qwen-plus" }]

[[routes."auto".tiers]]
name = "hard"
description = "Multi-step analysis and non-trivial code"
effort = "medium"
legs = [{ provider = "qwen", model = "qwen-max" }]

[routes."plain"]
legs = [{ provider = "qwen", model = "qwen-plus" }]
"#;

fn qwen_sse(text: &str) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .set_body_string(format!(
            "data: {{\"choices\":[{{\"delta\":{{\"content\":\"{text}\"}}}}]}}\n\n\
             data: {{\"choices\":[{{\"delta\":{{}},\"finish_reason\":\"stop\"}}],\"usage\":{{\"prompt_tokens\":3,\"completion_tokens\":5,\"total_tokens\":8}}}}\n\n\
             data: [DONE]\n\n"
        ))
}

fn jev_answers(score: f64, confidence: f64, needs_reasoning: f64) -> Value {
    json!({
        "model": "jev-1.13-20260917",
        "answers": {
            "difficulty": {"type": "score", "score": score, "legend": {}, "probabilities": {}, "confidence": confidence},
            "needs_reasoning": {"type": "noul", "noul": needs_reasoning}
        },
        "usage": {"input_tokens": 400, "output_tokens": 20},
        "provider": "TypeSafe"
    })
}

fn no_reasoning_effort(r: &wiremock::Request) -> bool {
    !String::from_utf8_lossy(&r.body).contains("reasoning_effort")
}

async fn jev_mock(template: ResponseTemplate, expect: u64) -> MockServer {
    let jev = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(template)
        .expect(expect)
        .mount(&jev)
        .await;
    jev
}

/// Answers `model` with SSE when the body also contains `needle`; expects `n` calls.
async fn qwen_serves(server: &MockServer, model: &str, needle: &str, text: &str, n: u64) {
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains(format!("\"model\":\"{model}\"")))
        .and(body_string_contains(needle.to_string()))
        .respond_with(qwen_sse(text))
        .expect(n)
        .mount(server)
        .await;
}

fn harness(
    routes_toml: &str,
    jev_uri: Option<String>,
    qwen_uri: &str,
    vertex_uri: Option<String>,
) -> (Gateway, Arc<InMemoryLedger>, impl Fn() -> String) {
    harness_guarded(
        routes_toml,
        jev_uri,
        qwen_uri,
        vertex_uri,
        GuardEngine::empty(),
    )
}

fn harness_guarded(
    routes_toml: &str,
    jev_uri: Option<String>,
    qwen_uri: &str,
    vertex_uri: Option<String>,
    guard: GuardEngine,
) -> (Gateway, Arc<InMemoryLedger>, impl Fn() -> String) {
    let routes = RouteTable::from_toml_str(routes_toml).unwrap();
    let env = HashMap::from([
        ("DASHSCOPE_API_KEY".to_string(), "sk-test".to_string()),
        ("DASHSCOPE_BASE_URL".to_string(), format!("{qwen_uri}/v1")),
        ("TYPESAFE_API_KEY".to_string(), "sk-test".to_string()),
        ("VERTEX_PROJECT_ID".to_string(), "p".to_string()),
    ]);
    let catalog =
        Catalog::build(&env, &routes.referenced_providers(), Duration::from_secs(5)).unwrap();
    let store = Arc::new(InMemoryLedger::default());
    let (metrics, exporter) = test_metrics();
    let gateway =
        Gateway::builder()
            .routes(routes)
            .catalog(catalog)
            .pricing(PricingTable::default())
            .ledger(LedgerHandle::spawn(
                store.clone() as Arc<dyn LedgerStore>,
                64,
            ))
            .metrics(metrics)
            .guard(guard)
            .jev_native(jev_uri.map(|uri| {
                JevNativeProvider::new("sk-test".into(), Some(uri), Duration::from_secs(5))
            }))
            .vertex_native(vertex_uri.map(|uri| {
                VertexNativeProvider::new(
                    Arc::new(VertexAuth::with_fetcher(|| {
                        Box::pin(async { Ok(("test-token".into(), Duration::from_secs(3600))) })
                    })),
                    "p".into(),
                    "global".into(),
                    Duration::from_secs(5),
                    Some(uri),
                )
            }))
            .build()
            .unwrap();
    (gateway, store, move || scrape(&exporter))
}

fn ask(model: &str, text: &str) -> Value {
    json!({"model": model, "messages": [{"role": "user", "content": text}]})
}

fn with(mut body: Value, extra: Value) -> Value {
    body.as_object_mut()
        .unwrap()
        .extend(extra.as_object().cloned().unwrap_or_default());
    body
}

async fn send(gw: Gateway, body: Value) -> (StatusCode, HeaderMap, String) {
    let resp = router(Arc::new(gw))
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("content-type", "application/json")
                .header("x-synapse-tenant", "acme")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        headers,
        String::from_utf8_lossy(&bytes).into_owned(),
    )
}

async fn wait_rows(store: &InMemoryLedger, n: usize) -> Vec<UsageEntry> {
    for _ in 0..200 {
        {
            let rows = store.entries.lock();
            if rows.len() >= n {
                return rows.clone();
            }
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    store.entries.lock().clone()
}

#[tokio::test]
async fn hard_decision_serves_the_hard_tier_with_its_effort() {
    let jev = jev_mock(
        ResponseTemplate::new(200).set_body_json(jev_answers(2.0, 0.9, 0.1)),
        1,
    )
    .await;
    let qwen = MockServer::start().await;
    qwen_serves(
        &qwen,
        "qwen-max",
        "\"reasoning_effort\":\"medium\"",
        "deep answer",
        1,
    )
    .await;

    let (gw, store, _) = harness(ROUTES, Some(jev.uri()), &qwen.uri(), None);
    let (status, _, body) = send(gw, ask("auto", "Refactor this module into three services")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let json: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["choices"][0]["message"]["content"], "deep answer");
    assert_eq!(json["model"], "qwen-max");

    let rows = wait_rows(&store, 2).await;
    let decision = rows
        .iter()
        .find(|r| r.op == "route_decision")
        .expect("decision row");
    assert_eq!(decision.provider, "typesafe");
    assert_eq!(decision.model, "jev-latest");
    assert_eq!(decision.lane, "jev");
    assert_eq!(decision.input_tokens, 400);
    assert_eq!(decision.route, "auto");
    assert_eq!(decision.tenant, "acme");
    assert_eq!(decision.score, Some(2.0));
    assert_eq!(decision.confidence, Some(0.9));
    assert_eq!(decision.tier.as_deref(), Some("hard"));
    assert_eq!(decision.outcome.as_deref(), Some("decided"));
    let chat = rows.iter().find(|r| r.op == "chat").expect("chat row");
    assert_eq!(chat.model, "qwen-max");
    assert_eq!(chat.request_id, decision.request_id);
    assert_eq!(chat.tenant, decision.tenant);
    assert_eq!(chat.score, None);
    assert_eq!(chat.tier, None);
}

#[tokio::test]
async fn needs_reasoning_bumps_effort_one_step() {
    let jev = jev_mock(
        ResponseTemplate::new(200).set_body_json(jev_answers(1.0, 0.9, 0.95)),
        1,
    )
    .await;
    let qwen = MockServer::start().await;
    qwen_serves(
        &qwen,
        "qwen-plus",
        "\"reasoning_effort\":\"medium\"",
        "ok",
        1,
    )
    .await;

    let (gw, _, _) = harness(ROUTES, Some(jev.uri()), &qwen.uri(), None);
    let (status, _, body) = send(gw, ask("auto", "Prove this sum telescopes")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn trivial_tier_with_effort_none_sends_no_reasoning_effort() {
    let jev = jev_mock(
        ResponseTemplate::new(200).set_body_json(jev_answers(0.0, 1.0, 0.0)),
        1,
    )
    .await;
    let qwen = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains("\"model\":\"qwen-flash\""))
        .and(no_reasoning_effort)
        .respond_with(qwen_sse("hi!"))
        .expect(1)
        .mount(&qwen)
        .await;

    let (gw, _, _) = harness(ROUTES, Some(jev.uri()), &qwen.uri(), None);
    let (status, _, body) = send(gw, ask("auto", "hello")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn jev_is_asked_about_bounded_state_against_tier_descriptions() {
    let jev = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .and(body_string_contains("\"model\":\"jev-latest\""))
        .and(body_string_contains("latest_user_message"))
        .and(body_string_contains(
            "Multi-step analysis and non-trivial code",
        ))
        .and(body_string_contains("needs_reasoning"))
        .respond_with(ResponseTemplate::new(200).set_body_json(jev_answers(1.0, 0.9, 0.0)))
        .expect(1)
        .mount(&jev)
        .await;
    let qwen = MockServer::start().await;
    qwen_serves(&qwen, "qwen-plus", "\"model\"", "ok", 1).await;

    let (gw, _, _) = harness(ROUTES, Some(jev.uri()), &qwen.uri(), None);
    let (status, _, body) = send(gw, ask("auto", "What's our refund policy?")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn static_override_skips_jev_and_serves_the_default_tier() {
    let jev = jev_mock(
        ResponseTemplate::new(200).set_body_json(jev_answers(2.0, 1.0, 0.0)),
        0,
    )
    .await;
    let qwen = MockServer::start().await;
    qwen_serves(&qwen, "qwen-plus", "\"reasoning_effort\":\"low\"", "ok", 1).await;

    let (gw, store, _) = harness(ROUTES, Some(jev.uri()), &qwen.uri(), None);
    let (status, _, body) = send(
        gw,
        with(
            ask("auto", "anything"),
            json!({"routing_strategy": "static"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let rows = settled_rows(&store, 1).await;
    assert!(rows.iter().all(|r| r.op != "route_decision"), "{rows:?}");
}

#[tokio::test]
async fn static_routes_are_unchanged() {
    let jev = jev_mock(
        ResponseTemplate::new(200).set_body_json(jev_answers(2.0, 1.0, 0.0)),
        0,
    )
    .await;
    let qwen = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains("\"model\":\"qwen-plus\""))
        .and(no_reasoning_effort)
        .respond_with(qwen_sse("plain"))
        .expect(1)
        .mount(&qwen)
        .await;

    let (gw, _, _) = harness(ROUTES, Some(jev.uri()), &qwen.uri(), None);
    let (status, _, body) = send(gw, ask("plain", "hi")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn invalid_routing_requests_are_400_before_any_upstream_call() {
    let cases = vec![
        (
            with(ask("auto", "x"), json!({"routing_strategy": "fastest"})),
            "unknown routing_strategy",
        ),
        (
            with(ask("plain", "x"), json!({"routing_strategy": "fastest"})),
            "unknown routing_strategy",
        ),
        (
            with(ask("plain", "x"), json!({"routing_strategy": "jev"})),
            "requires a route with tiers",
        ),
        (
            with(
                ask("auto", "x"),
                json!({"jev": {"questions": {"q": {"type": "noul", "instructions": "x"}}}}),
            ),
            "not supported on jev-routed routes",
        ),
        (
            with(
                ask("auto", "x"),
                json!({"vertex": {"response_schema": {"type": "object"}}}),
            ),
            "no legs of route 'auto' can serve",
        ),
    ];
    for (body, needle) in cases {
        let jev = jev_mock(
            ResponseTemplate::new(200).set_body_json(jev_answers(1.0, 1.0, 0.0)),
            0,
        )
        .await;
        let qwen = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(qwen_sse("must not run"))
            .expect(0)
            .mount(&qwen)
            .await;
        let (gw, _, _) = harness(ROUTES, Some(jev.uri()), &qwen.uri(), None);
        let (status, _, text) = send(gw, body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{text}");
        assert!(text.contains(needle), "expected '{needle}' in {text}");
    }
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

#[tokio::test]
async fn decided_response_carries_routing_headers() {
    let jev = jev_mock(
        ResponseTemplate::new(200).set_body_json(jev_answers(2.0, 0.9, 0.1)),
        1,
    )
    .await;
    let qwen = MockServer::start().await;
    qwen_serves(&qwen, "qwen-max", "\"model\"", "ok", 1).await;

    let (gw, _, _) = harness(ROUTES, Some(jev.uri()), &qwen.uri(), None);
    let (status, headers, body) = send(gw, ask("auto", "design a schema")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(header(&headers, "x-synapse-routing"), Some("jev"));
    assert_eq!(header(&headers, "x-synapse-tier"), Some("hard"));
    assert_eq!(
        header(&headers, "x-synapse-reasoning-effort"),
        Some("medium")
    );
    assert_eq!(header(&headers, "x-synapse-tier-decided"), None);
    assert_eq!(header(&headers, "x-synapse-routing-degraded"), None);
}

#[tokio::test]
async fn static_route_and_static_override_headers() {
    let jev = jev_mock(
        ResponseTemplate::new(200).set_body_json(jev_answers(2.0, 1.0, 0.0)),
        0,
    )
    .await;
    let qwen = MockServer::start().await;
    qwen_serves(&qwen, "qwen-plus", "\"model\"", "ok", 2).await;

    let (gw, _, _) = harness(ROUTES, Some(jev.uri()), &qwen.uri(), None);
    let (_, plain, _) = send(gw.clone(), ask("plain", "hi")).await;
    assert_eq!(header(&plain, "x-synapse-routing"), Some("static"));
    assert_eq!(header(&plain, "x-synapse-tier"), None);
    assert_eq!(header(&plain, "x-synapse-reasoning-effort"), None);

    let (_, over, _) = send(
        gw,
        with(ask("auto", "hi"), json!({"routing_strategy": "static"})),
    )
    .await;
    assert_eq!(header(&over, "x-synapse-routing"), Some("static-override"));
    assert_eq!(header(&over, "x-synapse-tier"), Some("moderate"));
    assert_eq!(header(&over, "x-synapse-reasoning-effort"), Some("low"));
}

#[tokio::test]
async fn client_reasoning_effort_is_forwarded_and_reported_as_client() {
    let jev = jev_mock(
        ResponseTemplate::new(200).set_body_json(jev_answers(0.0, 1.0, 0.0)),
        1,
    )
    .await;
    let qwen = MockServer::start().await;
    qwen_serves(
        &qwen,
        "qwen-flash",
        "\"reasoning_effort\":\"high\"",
        "ok",
        1,
    )
    .await;

    let (gw, _, _) = harness(ROUTES, Some(jev.uri()), &qwen.uri(), None);
    let (status, headers, body) = send(
        gw,
        with(ask("auto", "hi"), json!({"reasoning_effort": "high"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        header(&headers, "x-synapse-reasoning-effort"),
        Some("client")
    );
}

#[tokio::test]
async fn streaming_response_carries_routing_headers() {
    let jev = jev_mock(
        ResponseTemplate::new(200).set_body_json(jev_answers(2.0, 0.9, 0.1)),
        1,
    )
    .await;
    let qwen = MockServer::start().await;
    qwen_serves(&qwen, "qwen-max", "\"model\"", "streamed", 1).await;

    let (gw, _, _) = harness(ROUTES, Some(jev.uri()), &qwen.uri(), None);
    let (status, headers, body) = send(gw, with(ask("auto", "go"), json!({"stream": true}))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("streamed"), "{body}");
    assert_eq!(header(&headers, "x-synapse-routing"), Some("jev"));
    assert_eq!(header(&headers, "x-synapse-tier"), Some("hard"));
}

/// Rows once `n` have landed and any stragglers have had time to follow.
async fn settled_rows(store: &InMemoryLedger, n: usize) -> Vec<UsageEntry> {
    wait_rows(store, n).await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    store.entries.lock().clone()
}

fn assert_metric_line(text: &str, line: &str) {
    assert!(text.lines().any(|l| l == line), "no `{line}` in {text}");
}

#[tokio::test]
async fn jev_timeout_routes_to_default_tier_and_writes_no_decision_row() {
    let jev = jev_mock(
        ResponseTemplate::new(200)
            .set_body_json(jev_answers(2.0, 1.0, 0.0))
            .set_delay(Duration::from_millis(1_000)),
        1,
    )
    .await;
    let qwen = MockServer::start().await;
    qwen_serves(&qwen, "qwen-plus", "\"reasoning_effort\":\"low\"", "ok", 1).await;

    let routes = ROUTES.replace("timeout_ms = 2000", "timeout_ms = 200");
    let (gw, store, metrics) = harness(&routes, Some(jev.uri()), &qwen.uri(), None);
    let (status, headers, body) = send(gw, ask("auto", "hi")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(header(&headers, "x-synapse-tier"), Some("moderate"));
    assert_eq!(
        header(&headers, "x-synapse-routing-degraded"),
        Some("timeout")
    );
    let rows = settled_rows(&store, 1).await;
    assert!(rows.iter().all(|r| r.op != "route_decision"), "{rows:?}");
    assert_metric_line(
        &metrics(),
        r#"synapse_routing_decisions_total{outcome="timeout",route="auto",tier="moderate"} 1"#,
    );
}

#[tokio::test]
async fn low_confidence_routes_to_default_tier_but_still_records_the_decision() {
    let jev = jev_mock(
        ResponseTemplate::new(200).set_body_json(jev_answers(2.0, 0.2, 0.0)),
        1,
    )
    .await;
    let qwen = MockServer::start().await;
    qwen_serves(&qwen, "qwen-plus", "\"model\"", "ok", 1).await;

    let (gw, store, metrics) = harness(ROUTES, Some(jev.uri()), &qwen.uri(), None);
    let (status, headers, body) = send(gw, ask("auto", "hmm")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(header(&headers, "x-synapse-tier"), Some("moderate"));
    assert_eq!(
        header(&headers, "x-synapse-routing-degraded"),
        Some("low_confidence")
    );
    let rows = wait_rows(&store, 2).await;
    let decision = rows
        .iter()
        .find(|r| r.op == "route_decision")
        .expect("decision row");
    assert_eq!(decision.score, Some(2.0));
    assert_eq!(decision.confidence, Some(0.2));
    assert_eq!(decision.tier.as_deref(), Some("moderate"));
    assert_eq!(decision.outcome.as_deref(), Some("low_confidence"));
    assert_metric_line(
        &metrics(),
        r#"synapse_routing_decisions_total{outcome="low_confidence",route="auto",tier="moderate"} 1"#,
    );
}

#[tokio::test]
async fn jev_error_and_missing_provider_degrade_to_default_tier() {
    let jev = jev_mock(ResponseTemplate::new(500).set_body_string("boom"), 1).await;
    let qwen = MockServer::start().await;
    qwen_serves(&qwen, "qwen-plus", "\"model\"", "ok", 2).await;

    let (gw, _, metrics) = harness(ROUTES, Some(jev.uri()), &qwen.uri(), None);
    let (status, errored, body) = send(gw, ask("auto", "x")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(header(&errored, "x-synapse-tier"), Some("moderate"));
    assert_eq!(
        header(&errored, "x-synapse-routing-degraded"),
        Some("error")
    );
    assert_metric_line(
        &metrics(),
        r#"synapse_routing_decisions_total{outcome="error",route="auto",tier="moderate"} 1"#,
    );

    let (gw, _, _) = harness(ROUTES, None, &qwen.uri(), None);
    let (_, missing, _) = send(gw, ask("auto", "x")).await;
    assert_eq!(
        header(&missing, "x-synapse-routing-degraded"),
        Some("jev_unavailable")
    );
    assert_eq!(header(&missing, "x-synapse-tier"), Some("moderate"));
}

#[tokio::test]
async fn failed_tier_escalates_to_the_next_harder_tier() {
    let jev = jev_mock(
        ResponseTemplate::new(200).set_body_json(jev_answers(1.0, 0.9, 0.0)),
        1,
    )
    .await;
    let qwen = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains("\"model\":\"qwen-plus\""))
        .respond_with(ResponseTemplate::new(503).set_body_string("overloaded"))
        .expect(1..)
        .mount(&qwen)
        .await;
    qwen_serves(
        &qwen,
        "qwen-max",
        "\"reasoning_effort\":\"medium\"",
        "escalated",
        1,
    )
    .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains("\"model\":\"qwen-flash\""))
        .respond_with(qwen_sse("must not run"))
        .expect(0)
        .mount(&qwen)
        .await;

    let (gw, _, _) = harness(ROUTES, Some(jev.uri()), &qwen.uri(), None);
    let (status, headers, body) = send(gw, ask("auto", "x")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("escalated"), "{body}");
    assert_eq!(header(&headers, "x-synapse-tier"), Some("hard"));
    assert_eq!(header(&headers, "x-synapse-tier-decided"), Some("moderate"));
    assert_eq!(
        header(&headers, "x-synapse-reasoning-effort"),
        Some("medium")
    );
}

const VERTEX_ROUTES: &str = r#"
[routes."auto"]
strategy = "jev"
[routes."auto".jev_router]
default_tier = "moderate"
timeout_ms = 2000
[[routes."auto".tiers]]
name = "trivial"
description = "Greetings"
effort = "none"
legs = [{ provider = "qwen", model = "qwen-flash" }]
[[routes."auto".tiers]]
name = "moderate"
description = "Everyday questions"
effort = "low"
legs = [{ provider = "vertex", model = "gemini-2.5-flash" }]
[[routes."auto".tiers]]
name = "hard"
description = "Multi-step analysis"
effort = "medium"
legs = [{ provider = "vertex", model = "gemini-2.5-pro" }]
"#;

#[tokio::test]
async fn native_vertex_request_gets_a_thinking_budget_on_the_chosen_vertex_tier() {
    let jev = jev_mock(
        ResponseTemplate::new(200).set_body_json(jev_answers(2.0, 0.9, 0.0)),
        1,
    )
    .await;
    let vertex = MockServer::start().await;
    let sse = "data: {\"candidates\":[{\"finishReason\":\"STOP\",\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"{}\"}]}}],\"usageMetadata\":{\"promptTokenCount\":4,\"candidatesTokenCount\":6}}\n\n";
    Mock::given(method("POST"))
        .and(path(
            "/v1/projects/p/locations/global/publishers/google/models/gemini-2.5-pro:streamGenerateContent",
        ))
        .and(body_string_contains("\"thinkingBudget\":4096"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse),
        )
        .expect(1)
        .mount(&vertex)
        .await;
    let qwen = MockServer::start().await;

    let (gw, _, _) = harness(
        VERTEX_ROUTES,
        Some(jev.uri()),
        &qwen.uri(),
        Some(vertex.uri()),
    );
    let (status, headers, body) = send(
        gw,
        with(
            ask("auto", "extract"),
            json!({"vertex": {"response_schema": {"type": "object"}}}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(header(&headers, "x-synapse-tier"), Some("hard"));
    assert_eq!(
        header(&headers, "x-synapse-reasoning-effort"),
        Some("medium")
    );
}

#[tokio::test]
async fn native_vertex_request_ignores_client_reasoning_effort_and_keeps_the_tier_budget() {
    let jev = jev_mock(
        ResponseTemplate::new(200).set_body_json(jev_answers(2.0, 0.9, 0.0)),
        1,
    )
    .await;
    let vertex = MockServer::start().await;
    let sse = "data: {\"candidates\":[{\"finishReason\":\"STOP\",\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"{}\"}]}}],\"usageMetadata\":{\"promptTokenCount\":4,\"candidatesTokenCount\":6}}\n\n";
    Mock::given(method("POST"))
        .and(path(
            "/v1/projects/p/locations/global/publishers/google/models/gemini-2.5-pro:streamGenerateContent",
        ))
        .and(body_string_contains("\"thinkingBudget\":4096"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse),
        )
        .expect(1)
        .mount(&vertex)
        .await;
    let qwen = MockServer::start().await;

    let (gw, _, _) = harness(
        VERTEX_ROUTES,
        Some(jev.uri()),
        &qwen.uri(),
        Some(vertex.uri()),
    );
    let (status, headers, body) = send(
        gw,
        with(
            ask("auto", "extract"),
            json!({
                "reasoning_effort": "high",
                "vertex": {"response_schema": {"type": "object"}}
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(header(&headers, "x-synapse-tier"), Some("hard"));
    assert_eq!(
        header(&headers, "x-synapse-reasoning-effort"),
        Some("medium")
    );
}

#[tokio::test]
async fn routing_metrics_record_outcome_tier_and_latency() {
    let jev = jev_mock(
        ResponseTemplate::new(200).set_body_json(jev_answers(2.0, 0.9, 0.0)),
        1,
    )
    .await;
    let qwen = MockServer::start().await;
    qwen_serves(&qwen, "qwen-max", "\"model\"", "ok", 1).await;
    qwen_serves(&qwen, "qwen-plus", "\"model\"", "ok", 1).await;

    let (gw, _, metrics) = harness(ROUTES, Some(jev.uri()), &qwen.uri(), None);
    send(gw.clone(), ask("auto", "decide")).await;
    send(
        gw,
        with(ask("auto", "skip"), json!({"routing_strategy": "static"})),
    )
    .await;
    let text = metrics();
    let decided = text
        .lines()
        .find(|l| {
            l.starts_with("synapse_routing_decisions_total") && l.contains("outcome=\"decided\"")
        })
        .unwrap_or_else(|| panic!("no decided series in {text}"));
    assert!(
        decided.contains("tier=\"hard\"")
            && decided.contains("route=\"auto\"")
            && decided.ends_with(" 1"),
        "{decided}"
    );
    let skipped = text
        .lines()
        .find(|l| {
            l.starts_with("synapse_routing_decisions_total")
                && l.contains("outcome=\"static_override\"")
        })
        .unwrap_or_else(|| panic!("no static_override series in {text}"));
    assert!(skipped.contains("tier=\"moderate\""), "{skipped}");
    let count = text
        .lines()
        .find(|l| l.starts_with("synapse_routing_decision_duration_seconds_count"))
        .unwrap_or_else(|| panic!("no latency series in {text}"));
    assert!(
        count.ends_with(" 1"),
        "one Jev call, none for the override: {count}"
    );
}

#[tokio::test]
async fn static_override_alone_records_no_latency_sample() {
    let jev = jev_mock(
        ResponseTemplate::new(200).set_body_json(jev_answers(2.0, 0.9, 0.0)),
        0,
    )
    .await;
    let qwen = MockServer::start().await;
    qwen_serves(&qwen, "qwen-plus", "\"model\"", "ok", 1).await;

    let (gw, _, metrics) = harness(ROUTES, Some(jev.uri()), &qwen.uri(), None);
    send(
        gw,
        with(ask("auto", "skip"), json!({"routing_strategy": "static"})),
    )
    .await;
    assert!(!metrics().contains("synapse_routing_decision_duration_seconds"));
}

const GUARDED_ROUTES: &str = r#"
[routes."auto"]
strategy = "jev"
policy = "strict"
[routes."auto".jev_router]
default_tier = "moderate"
[[routes."auto".tiers]]
name = "moderate"
description = "Everyday questions"
effort = "low"
legs = [{ provider = "qwen", model = "qwen-plus" }]
[[routes."auto".tiers]]
name = "hard"
description = "Multi-step analysis"
effort = "medium"
legs = [{ provider = "qwen", model = "qwen-max" }]
"#;

#[tokio::test]
async fn guardrail_block_is_returned_before_jev_is_asked() {
    for body in [
        ask("auto", "this is forbidden"),
        with(ask("auto", "this is forbidden"), json!({"stream": true})),
    ] {
        let jev = jev_mock(
            ResponseTemplate::new(200).set_body_json(jev_answers(2.0, 1.0, 0.0)),
            0,
        )
        .await;
        let qwen = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(qwen_sse("must not run"))
            .expect(0)
            .mount(&qwen)
            .await;
        let guard = GuardEngine::from_config(
            &GuardrailsConfig::from_toml_str(
                r#"[guardrails.strict]
                   scanners = [{ type = "ban_substrings", substrings = ["forbidden"] }]"#,
            )
            .unwrap(),
        )
        .unwrap();

        let (gw, _, _) = harness_guarded(GUARDED_ROUTES, Some(jev.uri()), &qwen.uri(), None, guard);
        let (status, headers, text) = send(gw, body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{text}");
        let json: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(json["error"]["code"], "content_blocked", "{text}");
        assert_eq!(header(&headers, "x-synapse-routing"), None);
    }
}
