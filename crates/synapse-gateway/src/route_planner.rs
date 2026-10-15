//! Per-request route planning: a route's static legs, or — for
//! `strategy = "jev"` routes — the tier Jev rates the request at, with legs
//! ordered escalate-then-descend and stamped with reasoning effort. A Jev
//! failure never fails the request: it routes to `default_tier`.

use std::time::{Duration, Instant};

use chrono::Utc;
use serde_json::Value;
use tap::{Pipe, Tap, TapFallible};

use crate::error::GatewayError;
use crate::gateway::{Gateway, RequestCtx};
use crate::jev_native::JevNativeProvider;
use crate::ledger::UsageEntry;
use crate::routing::classify::vertex_triggers;
use crate::routing::jev_router::{
    self, Answers, DecisionOutcome, EffortPolicy, RoutePlan, RoutingMode, Selection,
};
use crate::routing::request::ChatRequest;
use crate::routing::table::JevRoute;

/// A parsed Jev decision plus the usage its ledger row records.
struct Decision {
    answers: Answers,
    input_tokens: u64,
    output_tokens: u64,
}

impl Gateway {
    /// Resolve the route, apply the `routing_strategy` override and the input
    /// guardrails, then plan legs: static routes as configured, `jev` routes by
    /// Jev's difficulty rating.
    pub(crate) async fn plan_route(
        &self,
        req: &mut ChatRequest,
        ctx: &RequestCtx,
        request_id: &str,
    ) -> Result<RoutePlan, GatewayError> {
        let legs = self
            .routes
            .legs(&req.model)
            .ok_or_else(|| GatewayError::UnknownModel(req.model.clone()))?;
        let jev_route = self.routes.jev_route(&req.model);
        let mode = jev_router::resolve_mode(jev_route.is_some(), req.routing_strategy.as_deref())
            .map_err(GatewayError::BadRequest)?;
        let decision = self.guard_input(req, ctx, request_id).await?;
        let plan = match (mode, jev_route) {
            (RoutingMode::Jev | RoutingMode::StaticOverride, Some(route)) => {
                self.plan_tiered(&*req, ctx, request_id, route, mode)
                    .await?
            }
            _ => RoutePlan::static_legs(legs, client_sets_effort(&*req)),
        };
        Ok(RoutePlan {
            guard: decision.masked.then_some("masked"),
            ..plan
        })
    }

    async fn plan_tiered(
        &self,
        req: &ChatRequest,
        ctx: &RequestCtx,
        request_id: &str,
        route: &JevRoute,
        mode: RoutingMode,
    ) -> Result<RoutePlan, GatewayError> {
        reject_jev_block(req)?;
        let tiers = jev_router::eligible_tiers(&route.tiers, vertex_triggers(req));
        jev_router::nearest_serving(&tiers, 0).ok_or_else(|| no_eligible_legs(req))?;
        let (selection, answers) = match mode {
            RoutingMode::StaticOverride => (
                Selection {
                    tier: route.default_index(),
                    outcome: DecisionOutcome::StaticOverride,
                    bump: false,
                },
                None,
            ),
            _ => self.decide(req, ctx, request_id, route).await,
        };
        let start = jev_router::nearest_serving(&tiers, selection.tier)
            .ok_or_else(|| no_eligible_legs(req))?;
        let policy = match client_sets_effort(req) {
            true => EffortPolicy::Client,
            false => EffortPolicy::Tier {
                bump: selection.bump,
            },
        };
        let decided = route.tiers[selection.tier].name.as_str();
        self.metrics
            .routing_decision(&req.model, decided, selection.outcome.metric_label());
        RoutePlan {
            mode,
            legs: jev_router::order_legs(&tiers, start, policy),
            tier_names: route.tiers.iter().map(|t| t.name.clone()).collect(),
            decided: Some(selection.tier),
            outcome: Some(selection.outcome),
            client_effort: matches!(policy, EffortPolicy::Client),
            guard: None,
        }
        .tap(|plan| {
            tracing::info!(
                target: "synapse::routing",
                route = %req.model,
                routing.mode = mode.as_str(),
                routing.tier_decided = decided,
                routing.outcome = selection.outcome.metric_label(),
                routing.effort = plan.planned_effort(),
                routing.difficulty_score = ?answers.map(|a| a.difficulty),
                routing.confidence = ?answers.map(|a| a.confidence),
                routing.needs_reasoning = ?answers.and_then(|a| a.needs_reasoning),
                "route planned"
            )
        })
        .pipe(Ok)
    }

    /// Ask Jev to rate the request under the route's timeout. Records latency
    /// for every call made and a ledger row for every parsed decision.
    async fn decide(
        &self,
        req: &ChatRequest,
        ctx: &RequestCtx,
        request_id: &str,
        route: &JevRoute,
    ) -> (Selection, Option<Answers>) {
        let result = match &self.jev_native {
            None => Err(DecisionOutcome::Unavailable),
            Some(provider) => {
                let started = Instant::now();
                let body = serde_json::json!({
                    "model": route.router.model,
                    "state": jev_router::build_state(req),
                    "questions": jev_router::build_questions(&route.tiers),
                });
                tokio::time::timeout(
                    Duration::from_millis(route.router.timeout_ms),
                    call_jev(provider, body, &req.model),
                )
                .await
                .tap_err(|_| {
                    tracing::warn!(
                        target: "synapse::routing",
                        route = %req.model,
                        error.kind = "timeout",
                        timeout_ms = route.router.timeout_ms,
                        "jev decision timed out; routing to default_tier"
                    )
                })
                .unwrap_or(Err(DecisionOutcome::Timeout))
                .tap(|_| {
                    self.metrics
                        .routing_decision_duration(&req.model, started.elapsed().as_secs_f64())
                })
            }
        }
        .tap_ok(|d| {
            self.record_route_decision(ctx, &req.model, request_id, &route.router.model, d)
        });
        let answers = result.as_ref().ok().map(|d| d.answers);
        (
            jev_router::select(result.map(|d| d.answers), route),
            answers,
        )
    }

    /// One ledger row per Jev decision, joined to the chat row by `request_id`.
    fn record_route_decision(
        &self,
        ctx: &RequestCtx,
        route: &str,
        request_id: &str,
        model: &str,
        d: &Decision,
    ) {
        let attr = self.attribution_of(ctx, route);
        self.ledger.enqueue(UsageEntry {
            ts: Utc::now(),
            tenant: self.tenant_of(ctx).to_string(),
            workspace: attr.workspace,
            user: attr.user,
            thread: attr.thread,
            message: attr.message,
            route: route.to_string(),
            provider: "typesafe".into(),
            model: model.to_string(),
            lane: "jev".into(),
            input_tokens: d.input_tokens,
            output_tokens: d.output_tokens,
            cost_usd: self
                .pricing
                .cost_usd("typesafe", model, d.input_tokens, d.output_tokens),
            request_id: request_id.to_string(),
            status: "ok".into(),
            op: "route_decision".into(),
            user_task_type: attr.user_task_type,
            ai_task_type: attr.ai_task_type,
        });
    }
}

/// Logs only the failure kind and HTTP status: Jev's body may echo customer
/// text.
async fn call_jev(
    provider: &JevNativeProvider,
    body: Value,
    route: &str,
) -> Result<Decision, DecisionOutcome> {
    let warn = |kind: &str, status: Option<u16>| {
        tracing::warn!(
            target: "synapse::routing",
            route,
            error.kind = kind,
            http.status = ?status,
            "jev decision failed; routing to default_tier"
        )
    };
    let resp = provider
        .evaluate(body)
        .await
        .tap_err(|_| warn("transport", None))
        .map_err(|_| DecisionOutcome::Error)?
        .error_for_status()
        .tap_err(|e| warn("http_status", e.status().map(|s| s.as_u16())))
        .map_err(|_| DecisionOutcome::Error)?;
    let status = resp.status().as_u16();
    let value: Value = resp
        .json()
        .await
        .tap_err(|_| warn("unreadable_body", Some(status)))
        .map_err(|_| DecisionOutcome::Error)?;
    jev_router::parse_answers(&value["answers"])
        .map(|answers| Decision {
            answers,
            input_tokens: value["usage"]["input_tokens"].as_u64().unwrap_or(0),
            output_tokens: value["usage"]["output_tokens"].as_u64().unwrap_or(0),
        })
        .ok_or(DecisionOutcome::Error)
        .tap_err(|_| warn("unparseable_answers", Some(status)))
}

/// The client chose its own effort on the lane that will serve it: a Vertex
/// `thinking_config` on the native lane (which never forwards
/// `reasoning_effort`), a parseable OpenAI `reasoning_effort` otherwise.
fn client_sets_effort(req: &ChatRequest) -> bool {
    match vertex_triggers(req) {
        true => req
            .vertex
            .as_ref()
            .is_some_and(|v| v.thinking_config.is_some()),
        false => crate::routing::executor::client_effort(req).is_some(),
    }
}

fn reject_jev_block(req: &ChatRequest) -> Result<(), GatewayError> {
    req.jev
        .as_ref()
        .is_some_and(|j| !j.questions.is_empty() || j.extract.is_some())
        .pipe(|has_block| match has_block {
            true => Err(GatewayError::BadRequest(
                "the jev extension is not supported on jev-routed routes".into(),
            )),
            false => Ok(()),
        })
}

fn no_eligible_legs(req: &ChatRequest) -> GatewayError {
    GatewayError::BadRequest(format!(
        "no legs of route '{}' can serve this request (native Vertex features need a vertex leg)",
        req.model
    ))
}
