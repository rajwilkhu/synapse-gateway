#![cfg(feature = "server")]

use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use synapse::embeddings::{EmbedOut, EmbeddingProvider};
use synapse::error::GatewayError;
use synapse::ledger::{InMemoryLedger, LedgerHandle, LedgerStore};
use synapse::pricing::PricingTable;
use synapse::providers::Catalog;
use synapse::routing::embeddings::EmbeddingRouteTable;
use synapse::routing::table::RouteTable;
use synapse::server::router;
use tower::ServiceExt;

/// One zero-vector per input (length `dims`) plus a fixed input-token count.
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

#[tokio::test]
async fn http_embeddings_returns_list_and_records_ledger() {
    let embed_routes = EmbeddingRouteTable::from_toml_str(
        r#"
        [embeddings."test-embed"]
        dimensions = 4
        legs = [{ provider = "stub", model = "stub-embed" }]
        "#,
    )
    .unwrap();

    // Chat routing is required by the builder even though we only exercise embeddings.
    let routes = RouteTable::from_toml_str(
        r#"[routes."fast"]
           legs = [{ provider = "qwen", model = "qwen-max" }]"#,
    )
    .unwrap();
    let catalog = Catalog::build(
        &std::collections::HashMap::from([("DASHSCOPE_API_KEY".to_string(), "sk".to_string())]),
        &routes.referenced_providers(),
        std::time::Duration::from_secs(5),
    )
    .unwrap();

    let store = Arc::new(InMemoryLedger::default());
    let ledger = LedgerHandle::spawn(store.clone() as Arc<dyn LedgerStore>, 64);
    let gateway = synapse::gateway::Gateway::builder()
        .routes(routes)
        .catalog(catalog)
        .pricing(PricingTable::default())
        .ledger(ledger)
        .default_tenant("unattributed")
        .embed_routes(embed_routes)
        .embedder("stub", Arc::new(StubEmbedder) as Arc<dyn EmbeddingProvider>)
        .build()
        .unwrap();

    let resp = router(Arc::new(gateway))
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/embeddings")
                .header("content-type", "application/json")
                .header("x-synapse-tenant", "acme")
                .body(Body::from(r#"{"input":["a","b"],"model":"test-embed"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["object"], "list");
    let data = v["data"].as_array().unwrap();
    assert_eq!(data.len(), 2);
    for (i, d) in data.iter().enumerate() {
        assert_eq!(d["index"], i);
        assert_eq!(d["embedding"].as_array().unwrap().len(), 4);
    }

    // ledger is fire-and-forget; wait briefly for the background writer to drain
    for _ in 0..50 {
        if store.entries.lock().len() == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    let entries = store.entries.lock();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].tenant, "acme");
    assert_eq!(entries[0].op, "embedding");
}

use synapse::guard::{GuardEngine, GuardrailsConfig};

fn guarded_embed_gateway(embed_toml: &str, guard_toml: &str) -> synapse::gateway::Gateway {
    let routes = RouteTable::from_toml_str(
        r#"[routes."fast"]
           legs = [{ provider = "qwen", model = "qwen-max" }]"#,
    )
    .unwrap();
    let catalog = Catalog::build(
        &std::collections::HashMap::from([("DASHSCOPE_API_KEY".to_string(), "sk".to_string())]),
        &routes.referenced_providers(),
        std::time::Duration::from_secs(5),
    )
    .unwrap();
    let store = Arc::new(InMemoryLedger::default());
    synapse::gateway::Gateway::builder()
        .routes(routes)
        .catalog(catalog)
        .pricing(PricingTable::default())
        .ledger(LedgerHandle::spawn(store as Arc<dyn LedgerStore>, 64))
        .embed_routes(EmbeddingRouteTable::from_toml_str(embed_toml).unwrap())
        .embedder("stub", Arc::new(StubEmbedder) as Arc<dyn EmbeddingProvider>)
        .guard(
            GuardEngine::from_config(&GuardrailsConfig::from_toml_str(guard_toml).unwrap())
                .unwrap(),
        )
        .build()
        .unwrap()
}

async fn post_embeddings(
    gw: synapse::gateway::Gateway,
    body: &str,
) -> (StatusCode, serde_json::Value) {
    let resp = router(Arc::new(gw))
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/embeddings")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

const EMBED: &str = r#"
    [embeddings."e"]
    dimensions = 4
    legs = [{ provider = "stub", model = "stub-embed" }]
    [embeddings."guarded"]
    dimensions = 4
    policy = "strict"
    legs = [{ provider = "stub", model = "stub-embed" }]
"#;

#[tokio::test]
async fn embeddings_scan_each_input_under_default() {
    let guard = r#"[guardrails.default]
        scanners = [{ type = "token_limit", max_chars = 10 }, { type = "ban_substrings", substrings = ["forbidden"] }]"#;
    let (ok, _) = post_embeddings(
        guarded_embed_gateway(EMBED, guard),
        r#"{"input":["123456","123456"],"model":"e"}"#,
    )
    .await;
    assert_eq!(ok, StatusCode::OK, "each input is under the limit");
    let (blocked, body) = post_embeddings(
        guarded_embed_gateway(EMBED, guard),
        r#"{"input":["fine","this is forbidden"],"model":"e"}"#,
    )
    .await;
    assert_eq!(blocked, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["type"], "content_policy_violation");
    assert!(!body.to_string().contains("this is forbidden"));
}

#[tokio::test]
async fn embedding_route_policy_overrides_default() {
    let guard = r#"
        [guardrails.default]
        scanners = [{ type = "ban_substrings", substrings = ["forbidden"] }]
        [guardrails.strict]
        scanners = [{ type = "ban_substrings", substrings = ["secret-word"] }]"#;
    let (s1, _) = post_embeddings(
        guarded_embed_gateway(EMBED, guard),
        r#"{"input":"forbidden","model":"guarded"}"#,
    )
    .await;
    assert_eq!(s1, StatusCode::OK, "guarded uses strict, not default");
    let (s2, _) = post_embeddings(
        guarded_embed_gateway(EMBED, guard),
        r#"{"input":"secret-word","model":"guarded"}"#,
    )
    .await;
    assert_eq!(s2, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn embeddings_without_any_policy_are_not_scanned() {
    let guard = r#"[guardrails.other]
        scanners = [{ type = "ban_substrings", substrings = ["forbidden"] }]"#;
    let (s, _) = post_embeddings(
        guarded_embed_gateway(EMBED, guard),
        r#"{"input":"forbidden","model":"e"}"#,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
}
