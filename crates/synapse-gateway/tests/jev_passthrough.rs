#![cfg(feature = "server")]

//! TypeSafe System One (Jev) passthrough surface: `POST /typesafe/v1/systemone`
//! forwards to TypeSafe's API and meters usage (tenant/workspace/user headers)
//! from `usage.{input,output}_tokens`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use synapse::gateway::Gateway;
use synapse::jev_native::JevNativeProvider;
use synapse::ledger::{InMemoryLedger, LedgerHandle, LedgerStore, UsageEntry};
use synapse::pricing::PricingTable;
use synapse::providers::Catalog;
use synapse::routing::table::RouteTable;
use synapse::server::router;
use tower::ServiceExt;
use wiremock::matchers::{body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const API_KEY: &str = "test-typesafe-key";
const MODEL: &str = "jev-latest";
const SYSTEMONE_PATH: &str = "/v1/systemone";

fn jev_gateway(mock_uri: Option<&str>) -> (Gateway, Arc<InMemoryLedger>) {
    // The passthrough lane never consults the route table; a dummy alias with
    // no legs keeps Catalog::build free of provider credentials.
    let routes = RouteTable::from_toml_str("[routes.\"dummy\"]\nlegs = []").unwrap();
    let catalog = Catalog::build(
        &HashMap::new(),
        &routes.referenced_providers(),
        Duration::from_secs(5),
    )
    .unwrap();
    let store = Arc::new(InMemoryLedger::default());
    let builder = Gateway::builder()
        .routes(routes)
        .catalog(catalog)
        .pricing(PricingTable::default())
        .ledger(LedgerHandle::spawn(
            store.clone() as Arc<dyn LedgerStore>,
            64,
        ))
        .default_tenant("unattributed");
    let gw = match mock_uri {
        Some(uri) => builder.jev_native(Some(JevNativeProvider::new(
            API_KEY.into(),
            Some(uri.to_string()),
            Duration::from_secs(5),
        ))),
        None => builder,
    }
    .build()
    .unwrap();
    (gw, store)
}

async fn ledger_rows(store: &InMemoryLedger) -> Vec<UsageEntry> {
    for _ in 0..100 {
        {
            let rows = store.entries.lock();
            if !rows.is_empty() {
                return rows.clone();
            }
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    store.entries.lock().clone()
}

fn jev_request(body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/typesafe/v1/systemone")
        .header("content-type", "application/json")
        .header("x-synapse-tenant", "acme")
        .header("x-synapse-workspace", "ws-9")
        .header("x-synapse-user", "user-42")
        .header("x-synapse-user-task-type", "triage")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn systemone_body() -> serde_json::Value {
    serde_json::json!({
        "state": "Stripe integration keeps failing, losing sales, help ASAP.",
        "questions": {
            "department": {
                "type": "choice",
                "instructions": "Which team should handle this",
                "criteria": {"billing": "Payment issues", "technical": "Integration problems"}
            }
        }
    })
}

#[tokio::test]
async fn systemone_forwards_and_meters() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(SYSTEMONE_PATH))
        .and(header("authorization", format!("Bearer {API_KEY}")))
        .and(body_string_contains("Stripe integration keeps failing"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "model": "jev-1.13.0",
            "answers": {
                "department": {
                    "type": "choice",
                    "choice": "technical",
                    "confidence": 0.78,
                    "probabilities": {"technical": 0.85, "billing": 0.15}
                }
            },
            "usage": {"input_tokens": 392, "output_tokens": 65}
        })))
        .mount(&mock)
        .await;

    let (gw, store) = jev_gateway(Some(&mock.uri()));
    let app = router(Arc::new(gw));

    let resp = app.oneshot(jev_request(systemone_body())).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    // Response is passed through verbatim (System One shape, not OpenAI).
    assert_eq!(json["answers"]["department"]["choice"], "technical");
    assert_eq!(json["model"], "jev-1.13.0");

    let rows = ledger_rows(&store).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].tenant, "acme");
    assert_eq!(rows[0].workspace.as_deref(), Some("ws-9"));
    assert_eq!(rows[0].user.as_deref(), Some("user-42"));
    assert_eq!(rows[0].user_task_type.as_deref(), Some("triage"));
    assert_eq!(rows[0].ai_task_type, "simple");
    assert_eq!(rows[0].provider, "typesafe");
    assert_eq!(rows[0].model, MODEL);
    // No route alias involved: the model id is the ledger route.
    assert_eq!(rows[0].route, MODEL);
    assert_eq!(rows[0].lane, "passthrough");
    assert_eq!(rows[0].op, "systemone");
    assert_eq!(rows[0].input_tokens, 392);
    assert_eq!(rows[0].output_tokens, 65);
    assert_eq!(rows[0].status, "ok");
}

#[tokio::test]
async fn model_defaults_to_jev_latest() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(SYSTEMONE_PATH))
        .and(body_string_contains(format!("\"model\":\"{MODEL}\"")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "model": "jev-1.13.0",
            "answers": {},
            "usage": {"input_tokens": 10, "output_tokens": 2}
        })))
        .mount(&mock)
        .await;

    let (gw, store) = jev_gateway(Some(&mock.uri()));
    let app = router(Arc::new(gw));

    let resp = app.oneshot(jev_request(systemone_body())).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let rows = ledger_rows(&store).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].model, MODEL);
    assert_eq!(rows[0].input_tokens, 10);
}

#[tokio::test]
async fn upstream_error_is_forwarded_verbatim_and_metered_as_error() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(SYSTEMONE_PATH))
        .respond_with(ResponseTemplate::new(422).set_body_json(serde_json::json!({
            "error": {"code": "invalid_question", "message": "question 'department' has no criteria"}
        })))
        .mount(&mock)
        .await;

    let (gw, store) = jev_gateway(Some(&mock.uri()));
    let app = router(Arc::new(gw));

    let resp = app.oneshot(jev_request(systemone_body())).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    // Upstream error body is not re-shaped into an OpenAI error.
    assert_eq!(json["error"]["code"], "invalid_question");

    let rows = ledger_rows(&store).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, "error");
    assert_eq!(rows[0].input_tokens, 0);
    assert_eq!(rows[0].output_tokens, 0);
}

#[tokio::test]
async fn lane_unconfigured_returns_bad_request() {
    let (gw, store) = jev_gateway(None);
    let app = router(Arc::new(gw));

    let resp = app.oneshot(jev_request(systemone_body())).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"]["code"], "invalid_request_error");

    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(store.entries.lock().is_empty());
}

use synapse::guard::{GuardEngine, GuardrailsConfig};

fn guarded_jev_gateway(mock_uri: &str, guard_toml: &str) -> Gateway {
    let routes = RouteTable::from_toml_str("[routes.\"dummy\"]\nlegs = []").unwrap();
    let catalog = Catalog::build(
        &HashMap::new(),
        &routes.referenced_providers(),
        Duration::from_secs(5),
    )
    .unwrap();
    Gateway::builder()
        .routes(routes)
        .catalog(catalog)
        .pricing(PricingTable::default())
        .ledger(LedgerHandle::spawn(
            Arc::new(InMemoryLedger::default()) as Arc<dyn LedgerStore>,
            64,
        ))
        .jev_native(Some(JevNativeProvider::new(
            API_KEY.into(),
            Some(mock_uri.to_string()),
            Duration::from_secs(5),
        )))
        .guard(
            GuardEngine::from_config(&GuardrailsConfig::from_toml_str(guard_toml).unwrap())
                .unwrap(),
        )
        .build()
        .unwrap()
}

#[tokio::test]
async fn jev_passthrough_blocks_on_default_before_any_upstream_call() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(SYSTEMONE_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"answers": {}})))
        .expect(0)
        .mount(&mock)
        .await;
    let gw = guarded_jev_gateway(
        &mock.uri(),
        r#"[guardrails.default]
           scanners = [{ type = "ban_substrings", substrings = ["forbidden"] }]"#,
    );
    let body = serde_json::json!({"state": {"ticket": {"text": "this is forbidden"}, "n": 1}, "questions": {}});
    let resp = router(Arc::new(gw))
        .oneshot(jev_request(body))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["error"]["type"], "content_policy_violation");
}

#[tokio::test]
async fn jev_passthrough_does_not_scan_numbers_or_keys() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(SYSTEMONE_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            serde_json::json!({"answers": {}, "usage": {"input_tokens": 1, "output_tokens": 1}}),
        ))
        .expect(1)
        .mount(&mock)
        .await;
    let gw = guarded_jev_gateway(
        &mock.uri(),
        r#"[guardrails.default]
           scanners = [{ type = "ban_substrings", substrings = ["forbidden", "1234"] }]"#,
    );
    let body = serde_json::json!({"state": {"forbidden": 1234}, "questions": {}});
    let resp = router(Arc::new(gw))
        .oneshot(jev_request(body))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}
