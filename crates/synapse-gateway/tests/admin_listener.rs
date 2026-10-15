#![cfg(feature = "server")]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use synapse::gateway::Gateway;
use synapse::ledger::{InMemoryLedger, LedgerHandle, LedgerStore};
use synapse::pricing::PricingTable;
use synapse::providers::Catalog;
use synapse::routing::table::RouteTable;
use synapse::server::apps;
use tower::ServiceExt;

fn gateway() -> Arc<Gateway> {
    let routes = RouteTable::from_toml_str("[routes.\"dummy\"]\nlegs = []").unwrap();
    let catalog = Catalog::build(
        &HashMap::new(),
        &routes.referenced_providers(),
        Duration::from_secs(5),
    )
    .unwrap();
    Arc::new(
        Gateway::builder()
            .routes(routes)
            .catalog(catalog)
            .pricing(PricingTable::default())
            .ledger(LedgerHandle::spawn(
                Arc::new(InMemoryLedger::default()) as Arc<dyn LedgerStore>,
                16,
            ))
            .build()
            .unwrap(),
    )
}

fn register() -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/internal/a2a/agents")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({
                "id": "agent-1", "name": "Agent", "description": "d",
                "endpoint_url": "http://a/rpc", "card_url": "http://a/card",
                "tags": [], "card": {}
            })
            .to_string(),
        ))
        .unwrap()
}

#[tokio::test]
async fn admin_routes_leave_the_api_app_unless_shared() {
    let registry = Arc::new(synapse_a2a::A2aRegistry::new());
    let (api, admin) = apps(gateway(), registry.clone(), false);
    assert_eq!(
        api.oneshot(register()).await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
    let admin = admin.expect("separate admin app");
    assert_eq!(
        admin.oneshot(register()).await.unwrap().status(),
        StatusCode::NO_CONTENT
    );

    let (shared, none) = apps(gateway(), Arc::new(synapse_a2a::A2aRegistry::new()), true);
    assert!(none.is_none());
    assert_eq!(
        shared.oneshot(register()).await.unwrap().status(),
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn public_catalog_stays_on_the_api_app() {
    let (api, _) = apps(gateway(), Arc::new(synapse_a2a::A2aRegistry::new()), false);
    let resp = api
        .oneshot(
            Request::builder()
                .uri("/.well-known/a2a-agent-catalog.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}
