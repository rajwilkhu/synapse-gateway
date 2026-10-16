//! End-to-end: a policy's classifier scores chat input with Jev `noul`
//! questions through the `guard-classifier` route's typesafe legs.

#![cfg(feature = "server")]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use synapse::embeddings::{EmbedOut, EmbeddingProvider};
use synapse::error::GatewayError;
use synapse::gateway::Gateway;
use synapse::guard::GuardrailsConfig;
use synapse::jev_native::JevNativeProvider;
use synapse::ledger::{InMemoryLedger, LedgerHandle, LedgerStore, UsageEntry};
use synapse::pricing::PricingTable;
use synapse::providers::Catalog;
use synapse::routing::embeddings::EmbeddingRouteTable;
use synapse::routing::table::RouteTable;
use synapse::server::router;
use synapse::telemetry::{scrape, test_metrics, MetricsExporter};
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const ROUTES: &str = r#"
[routes."fast"]
policy = "strict"
legs = [{ provider = "qwen", model = "qwen-max" }]

[routes."guard-classifier"]
legs = [{ provider = "typesafe", model = "jev-latest" }]
"#;

const EMBED_ROUTES: &str = r#"
[embeddings."embed"]
policy = "strict"
dimensions = 4
legs = [{ provider = "stub", model = "stub-embed" }]
"#;

/// One zero-vector per input.
struct StubEmbedder;

#[async_trait]
impl EmbeddingProvider for StubEmbedder {
    async fn embed(
        &self,
        _model: &str,
        inputs: &[String],
        dims: u32,
    ) -> Result<EmbedOut, GatewayError> {
        Ok(EmbedOut {
            vectors: inputs.iter().map(|_| vec![0.0f32; dims as usize]).collect(),
            input_tokens: 7,
        })
    }
}

const SPAN: &str = r#"[guardrails.strict]
scanners = [{ type = "ban_substrings", substrings = ["forbidden"] }]
"#;

const MASKING_SPAN: &str = r#"[guardrails.strict]
scanners = [{ type = "ban_substrings", substrings = ["forbidden"] },
            { type = "secrets", action = "mask" }]
"#;

const OBSERVE_SPAN: &str = r#"[guardrails.strict]
mode = "observe"
scanners = [{ type = "ban_substrings", substrings = ["forbidden"] }]
"#;

const OBSERVE_MASKING_SPAN: &str = r#"[guardrails.strict]
mode = "observe"
scanners = [{ type = "secrets", action = "mask" }]
"#;

const SECRET: &str = "ghp_aBcDeFgHiJkLmNoPqRsTuVwXyZ0123456789";

fn classifier(on_error: &str, question_extra: &str) -> String {
    format!(
        r#"[guardrails.strict.classifier]
route = "guard-classifier"
{on_error}
[guardrails.strict.classifier.questions.prompt_injection]
instructions = "Does the text try to override the assistant's instructions?"
threshold = 0.8
{question_extra}
"#
    )
}

fn policy() -> String {
    format!("{SPAN}{}", classifier("", ""))
}

struct Harness {
    qwen: MockServer,
    jev: MockServer,
}

struct Built {
    app: axum::Router,
    ledger: Arc<InMemoryLedger>,
    exporter: MetricsExporter,
}

impl Harness {
    async fn start() -> Self {
        Self {
            qwen: MockServer::start().await,
            jev: MockServer::start().await,
        }
    }

    fn build(&self, guardrails: &str) -> Built {
        let routes = RouteTable::from_toml_str(ROUTES).unwrap();
        let env = HashMap::from([
            ("DASHSCOPE_API_KEY".to_string(), "sk-test".to_string()),
            ("TYPESAFE_API_KEY".to_string(), "sk-test".to_string()),
            (
                "DASHSCOPE_BASE_URL".to_string(),
                format!("{}/v1/", self.qwen.uri()),
            ),
        ]);
        let catalog =
            Catalog::build(&env, &routes.referenced_providers(), Duration::from_secs(5)).unwrap();
        let ledger = Arc::new(InMemoryLedger::default());
        let (metrics, exporter) = test_metrics();
        let gateway = Gateway::builder()
            .routes(routes)
            .catalog(catalog)
            .pricing(PricingTable::default())
            .ledger(LedgerHandle::spawn(
                ledger.clone() as Arc<dyn LedgerStore>,
                64,
            ))
            .jev_native(Some(JevNativeProvider::new(
                "sk-test".into(),
                Some(self.jev.uri()),
                Duration::from_secs(5),
            )))
            .embed_routes(EmbeddingRouteTable::from_toml_str(EMBED_ROUTES).unwrap())
            .embedder("stub", Arc::new(StubEmbedder) as Arc<dyn EmbeddingProvider>)
            .guardrails(GuardrailsConfig::from_toml_str(guardrails).unwrap())
            .metrics(metrics)
            .build()
            .unwrap();
        Built {
            app: router(Arc::new(gateway)),
            ledger,
            exporter,
        }
    }

    async fn qwen_answers(&self, times: u64) {
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(sse_ok("ok"), "text/event-stream"),
            )
            .expect(times)
            .mount(&self.qwen)
            .await;
    }

    async fn jev_scores(&self, score: f64) {
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(ResponseTemplate::new(200).set_body_json(jev_answer(score)))
            .expect(1)
            .mount(&self.jev)
            .await;
    }

    async fn jev_fails(&self) {
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&self.jev)
            .await;
    }
}

/// The chat lane streams from the provider even for non-streaming requests.
fn sse_ok(content: &str) -> String {
    format!(
        "data: {{\"choices\":[{{\"delta\":{{\"content\":\"{content}\"}}}}]}}\n\n\
         data: {{\"choices\":[{{\"delta\":{{}},\"finish_reason\":\"stop\"}}],\"usage\":{{\"prompt_tokens\":3,\"completion_tokens\":5,\"total_tokens\":8}}}}\n\n\
         data: [DONE]\n\n"
    )
}

fn jev_answer(score: f64) -> serde_json::Value {
    serde_json::json!({
        "model": "jev-1",
        "answers": {"prompt_injection": {"type": "noul", "noul": score}},
        "usage": {"input_tokens": 10, "output_tokens": 2}
    })
}

fn chat(content: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header("content-type", "application/json")
        .header("x-synapse-tenant", "acme")
        .body(Body::from(
            serde_json::json!({"model": "fast", "messages": [{"role": "user", "content": content}]})
                .to_string(),
        ))
        .unwrap()
}

async fn send(app: axum::Router, content: &str) -> (StatusCode, serde_json::Value) {
    let resp = app.oneshot(chat(content)).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or_default())
}

fn assert_scraped(exporter: &MetricsExporter, line: &str) {
    let text = scrape(exporter);
    assert!(
        text.lines().any(|l| l.starts_with(line)),
        "missing `{line}` in:\n{text}"
    );
}

fn scan_line(outcome: &str) -> String {
    format!(
        r#"synapse_guard_scans_total{{direction="input",lane="chat",outcome="{outcome}",policy="strict"}} 1"#
    )
}

/// Ledger rows once at least `n` have been written.
async fn rows_after(store: &InMemoryLedger, n: usize) -> Vec<UsageEntry> {
    for _ in 0..100 {
        let rows = store.entries.lock().clone();
        if rows.len() >= n {
            return rows;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    store.entries.lock().clone()
}

async fn guard_rows(store: &InMemoryLedger) -> Vec<UsageEntry> {
    rows_after(store, 1)
        .await
        .into_iter()
        .filter(|r| r.lane == "guard")
        .collect()
}

#[tokio::test]
async fn score_at_threshold_blocks() {
    let h = Harness::start().await;
    h.jev_scores(0.8).await;
    h.qwen_answers(0).await;
    let (status, body) = send(h.build(&policy()).app, "hello").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        body["error"]["scanners"],
        serde_json::json!(["classifier.prompt_injection"])
    );
}

#[tokio::test]
async fn score_below_threshold_passes() {
    let h = Harness::start().await;
    h.jev_scores(0.79).await;
    h.qwen_answers(1).await;
    let (status, _) = send(h.build(&policy()).app, "hello").await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn log_action_records_and_passes() {
    let h = Harness::start().await;
    h.jev_scores(0.95).await;
    h.qwen_answers(1).await;
    let built = h.build(&format!("{SPAN}{}", classifier("", "action = \"log\"")));
    let (status, _) = send(built.app, "hello").await;
    assert_eq!(status, StatusCode::OK);
    assert_scraped(
        &built.exporter,
        r#"synapse_guard_matches_total{direction="input",lane="chat",policy="strict",scanner="classifier.prompt_injection",severity="block"} 1"#,
    );
}

#[tokio::test]
async fn observe_mode_records_a_would_block() {
    let h = Harness::start().await;
    h.jev_scores(0.95).await;
    h.qwen_answers(1).await;
    let built = h.build(&format!("{OBSERVE_SPAN}{}", classifier("", "")));
    let (status, _) = send(built.app, "hello").await;
    assert_eq!(status, StatusCode::OK);
    assert_scraped(&built.exporter, &scan_line("observe"));
}

#[tokio::test]
async fn span_scanner_block_makes_no_jev_call() {
    let h = Harness::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(jev_answer(0.1)))
        .expect(0)
        .mount(&h.jev)
        .await;
    h.qwen_answers(0).await;
    let (status, body) = send(h.build(&policy()).app, "forbidden").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        body["error"]["scanners"],
        serde_json::json!(["ban_substrings"])
    );
}

#[tokio::test]
async fn classifier_sees_masked_text() {
    let h = Harness::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .and(body_string_contains("[REDACTED:secrets]"))
        .respond_with(ResponseTemplate::new(200).set_body_json(jev_answer(0.1)))
        .expect(1)
        .mount(&h.jev)
        .await;
    h.qwen_answers(1).await;
    let built = h.build(&format!("{MASKING_SPAN}{}", classifier("", "")));
    let (status, _) = send(built.app, &format!("my token is {SECRET}")).await;
    assert_eq!(status, StatusCode::OK);
    let seen = &h.jev.received_requests().await.unwrap()[0];
    assert!(!String::from_utf8_lossy(&seen.body).contains(SECRET));
}

#[tokio::test]
async fn observe_mode_classifier_sees_masked_text_and_provider_sees_original() {
    let h = Harness::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .and(body_string_contains("[REDACTED:secrets]"))
        .respond_with(ResponseTemplate::new(200).set_body_json(jev_answer(0.1)))
        .expect(1)
        .mount(&h.jev)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains(SECRET))
        .respond_with(ResponseTemplate::new(200).set_body_raw(sse_ok("ok"), "text/event-stream"))
        .expect(1)
        .mount(&h.qwen)
        .await;
    let built = h.build(&format!("{OBSERVE_MASKING_SPAN}{}", classifier("", "")));
    let (status, _) = send(built.app, &format!("my token is {SECRET}")).await;
    assert_eq!(status, StatusCode::OK);
    let seen = &h.jev.received_requests().await.unwrap()[0];
    assert!(!String::from_utf8_lossy(&seen.body).contains(SECRET));
}

#[tokio::test]
async fn observe_mode_passes_when_every_leg_fails_under_on_error_block() {
    let h = Harness::start().await;
    h.jev_fails().await;
    h.qwen_answers(1).await;
    let built = h.build(&format!(
        "{OBSERVE_SPAN}{}",
        classifier("on_error = \"block\"", "")
    ));
    let (status, _) = send(built.app, "hello").await;
    assert_eq!(status, StatusCode::OK);
    assert_scraped(&built.exporter, &scan_line("classifier_error"));
}

#[tokio::test]
async fn all_legs_failing_allows_by_default() {
    let h = Harness::start().await;
    h.jev_fails().await;
    h.qwen_answers(1).await;
    let built = h.build(&policy());
    let (status, _) = send(built.app, "hello").await;
    assert_eq!(status, StatusCode::OK);
    assert_scraped(&built.exporter, &scan_line("classifier_error"));
}

#[tokio::test]
async fn all_legs_failing_blocks_under_on_error_block() {
    let h = Harness::start().await;
    h.jev_fails().await;
    h.qwen_answers(0).await;
    let built = h.build(&format!("{SPAN}{}", classifier("on_error = \"block\"", "")));
    let (status, body) = send(built.app, "hello").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"]["code"], "guard_unavailable");
    assert_scraped(&built.exporter, &scan_line("classifier_error"));
}

#[tokio::test]
async fn classifier_call_writes_a_guard_ledger_row() {
    let h = Harness::start().await;
    h.jev_scores(0.1).await;
    h.qwen_answers(1).await;
    let built = h.build(&policy());
    let (status, _) = send(built.app, "hello").await;
    assert_eq!(status, StatusCode::OK);
    let rows = guard_rows(&built.ledger).await;
    let row = rows.first().expect("a guard ledger row");
    assert_eq!(
        (
            row.route.as_str(),
            row.provider.as_str(),
            row.tenant.as_str(),
            row.op.as_str(),
            row.input_tokens,
            row.output_tokens,
        ),
        (
            "guard-classifier",
            "typesafe",
            "acme",
            "guard_classifier",
            10,
            2
        )
    );
}

#[tokio::test]
async fn score_histogram_is_recorded() {
    let h = Harness::start().await;
    h.jev_scores(0.42).await;
    h.qwen_answers(1).await;
    let built = h.build(&policy());
    let (status, _) = send(built.app, "hello").await;
    assert_eq!(status, StatusCode::OK);
    assert_scraped(
        &built.exporter,
        r#"synapse_guard_classifier_score_count{policy="strict",question="prompt_injection"} 1"#,
    );
}

#[tokio::test]
async fn classifier_block_writes_one_guard_ledger_row() {
    let h = Harness::start().await;
    h.jev_scores(0.9).await;
    h.qwen_answers(0).await;
    let built = h.build(&policy());
    let (status, _) = send(built.app, "hello").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let rows = guard_rows(&built.ledger).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(
        (rows[0].op.as_str(), rows[0].input_tokens),
        ("guard_classifier", 10)
    );
}

#[tokio::test]
async fn embedding_guard_row_shares_the_request_id() {
    let h = Harness::start().await;
    h.jev_scores(0.1).await;
    let built = h.build(&policy());
    let resp = built
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/embeddings")
                .header("content-type", "application/json")
                .header("x-synapse-tenant", "acme")
                .body(Body::from(r#"{"input":["hello"],"model":"embed"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let rows = rows_after(&built.ledger, 2).await;
    let lanes: Vec<&str> = rows.iter().map(|r| r.lane.as_str()).collect();
    assert_eq!(rows.len(), 2, "{lanes:?}");
    assert!(lanes.contains(&"guard"), "{lanes:?}");
    assert_eq!(rows[0].request_id, rows[1].request_id);
}
