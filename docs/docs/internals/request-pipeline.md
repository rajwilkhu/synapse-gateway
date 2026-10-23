---
sidebar_position: 1
title: Request pipeline
description: How a chat completion moves through the gateway's source code, step by step, from the HTTP handler to the ledger write.
---

This page follows one `POST /v1/chat/completions` request through the `synapse-gateway` source,
in the order the code runs it, with a link to the file for each step. Read it when you want to
change the gateway, debug a surprising response, or check which step produced an error. For
what each step means to a client, see [Architecture](../overview/architecture.md).

All paths are under
[`crates/synapse-gateway/src`](https://github.com/sustentabilitas/synapse-gateway/tree/main/crates/synapse-gateway/src).

```text
server.rs            parse JSON, read x-synapse-* headers, pick a request id
  │
gateway.rs           chat_routed (buffered) or chat_stream (streaming)
  │
route_planner.rs     look up the alias → routing_strategy → guardrails → legs or Jev decision
  │
gateway.rs           jev block checks, extract spec checks, hybrid branch
  │
routing/classify.rs  standard, native Vertex or Jev lane
  │
  ├── routing/executor.rs   standard lane: walk legs through genai
  ├── vertex_native.rs      native Vertex lane: walk vertex legs
  └── jev_native.rs         Jev lane: walk typesafe legs, then fall back to chat legs
  │
server.rs            render JSON or server-sent events, add routing headers
  │
gateway.rs           record(), or StreamSideEffects on drop
  ├── ledger/        queue the usage row; a background task writes it to each sink
  └── telemetry.rs   count the request and its tokens
```

## 1. HTTP handler

[`server.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/server.rs)
builds the axum router and handles `POST /v1/chat/completions` in `chat_completions`.

- axum's `Json` extractor parses the body into a `ChatRequest`, defined in
  [`routing/request.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/routing/request.rs).
  A body that doesn't parse is rejected here, with axum's plain-text error rather than the
  gateway's JSON error. Fields the struct doesn't name are collected into `passthrough`.
- `request_ctx` reads the seven `x-synapse-*` headers into a `RequestCtx`.
- The request id is the `x-synapse-message` header when it is present and non-empty, otherwise
  a new UUID. It becomes the response `id` and the ledger's `request_id`.
- `stream: true` goes to `Gateway::chat_stream`; anything else to `Gateway::chat_routed`.

## 2. Gateway entry

[`gateway.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/gateway.rs)
holds `Gateway`, the in-process gateway that the HTTP layer and
[embedding applications](../guides/embedding-as-library.md) both call. `chat_routed` and
`chat_stream` run the same steps up to execution; they differ in how they execute legs and when
they record usage. Both start a timer here, so the latency metric includes planning and the Jev
decision.

## 3. Route planning

[`route_planner.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/route_planner.rs)
turns the request into a `RoutePlan`, an ordered list of legs, in `plan_route`:

1. **Look up the alias** in the `RouteTable` from
   [`routing/table.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/routing/table.rs).
   An unknown alias fails with `404 model_not_found`.
2. **Resolve `routing_strategy`** with `resolve_mode` in
   [`routing/jev_router.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/routing/jev_router.rs).
   An invalid value fails with `400`.
3. **Run guardrails** through `Gateway::guard_input`, which picks the route's `policy` or
   `default` and calls
   [`guard/engine.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/guard/engine.rs).
   The policies come from
   [`guard/policy.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/guard/policy.rs),
   the scanners from
   [`guard/scanners.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/guard/scanners.rs),
   in-place `[REDACTED:<scanner>]` span masking from
   [`guard/mask.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/guard/mask.rs),
   and Jev semantic `noul` classification (running after span scanners pass on the post-mask
   text) from
   [`guard/classifier.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/guard/classifier.rs).
   Non-`pass` scans emit a structured `synapse::guard` log event; an answered classifier check
   or an input block writes a `lane = "guard"` ledger row (`guard_classifier` or `guard_input`),
   and a block fails with `400 content_blocked` before any provider or Jev router call.
4. **Choose the legs.** A static route uses its legs in order. A `strategy = "jev"` route goes
   through `plan_tiered`:
   - a request with a `jev` block fails with `400`;
   - for a native Vertex request, only `vertex` legs are kept, and tiers left without legs are
     skipped;
   - `decide` calls Jev with the route's `timeout_ms`, or skips the call for
     `"routing_strategy": "static"`. A parsed decision writes its own ledger row;
   - `jev_router.rs` selects the tier and effort and orders the legs: the chosen tier, each
     harder tier, then each easier tier;
   - the decision is counted in `synapse_routing_decisions_total` and logged as `route planned`
     with target `synapse::routing`.

A Jev problem never fails planning; it falls back to `default_tier`. See
[Jev router](../guides/jev-router.md#failure-behaviour).

## 4. Request checks

Back in `gateway.rs`:

- `require_jev_block` fails with `400` when the chain has `typesafe` legs and the request has
  no `jev` questions.
- `validate_extract` in
  [`routing/jev_extract.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/routing/jev_extract.rs)
  checks a hybrid extraction spec.
- A buffered request with `jev.extract` leaves the pipeline here for `chat_hybrid` in
  [`jev_hybrid.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/jev_hybrid.rs),
  which asks Jev, then runs one extraction per surviving candidate on the chat legs and records
  a ledger row for each.

## 5. Lane classification

`classify` in
[`routing/classify.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/routing/classify.rs)
picks the lane from the body alone: a `jev` block with questions means the Jev lane; a `vertex`
block with `cached_content`, `response_schema`, `thinking_config` or a `gs://` media URI means
the native Vertex lane; anything else, the standard lane. See
[Lane detection](../overview/architecture.md#lane-detection).

## 6. Execution

Each lane walks the plan's legs differently.

**Standard lane.**
[`routing/executor.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/routing/executor.rs)
calls providers through the `genai` crate, using the provider catalogue built in
[`providers/mod.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/providers/mod.rs)
and
[`providers/genai_provider.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/providers/genai_provider.rs),
with per-leg retries and circuit breakers from
[`resilience.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/resilience.rs).
`to_genai_options` builds the per-request options, which is where fields are forwarded or
dropped.

- `execute_buffered_with_timeouts` buffers each leg's whole stream, with the first-chunk and
  idle timeouts, and moves to the next leg on any failure.
- `execute_streaming_with_timeouts` opens each leg and waits for its first chunk. The first leg
  to produce one is committed; later failures reach the client as an error event.
- Both return `502 all_legs_failed` with every leg's failure when the chain runs out, or
  `503 all_circuits_open` when every leg's circuit breaker is open.

**Native Vertex lane.** `Gateway::native_committed` walks the chain's `vertex` legs, turns each
leg's effort into a thinking budget with `with_leg_thinking`, and calls `stream_generate` in
[`vertex_native.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/vertex_native.rs),
which translates the request to Vertex's format and calls the leg's region. A `5xx`, `429`,
`408` or connection error moves to the next leg; any other `4xx` stops the chain. For a
buffered request, `collect_committed` drains the committed stream into one completion. Google
credentials come from
[`providers/vertex_auth.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/providers/vertex_auth.rs).

**Jev lane.** `Gateway::jev_attempt` sends the questions to each `typesafe` leg through
[`jev_native.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/jev_native.rs).
When every leg fails retryably, the remaining legs answer as a chat completion on the native
Vertex or standard lane.

The provider HTTP timeout, `SYNAPSE_REQUEST_TIMEOUT_SECS`, is set on each provider's client
when the gateway is built in
[`main.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/main.rs).

## 7. Response

`server.rs` renders the outcome, after optional `output_scanners` in
[`guard/output.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/guard/output.rs)
screen buffered replies or hold back `output_holdback_chars` on streams:

- `openai_json` builds a `chat.completion` through the `Accumulator` in
  [`routing/stream.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/routing/stream.rs),
  and `hybrid_json` the hybrid envelope.
- `sse_body` renders a stream as `chat.completion.chunk` events with `stream_item_to_sse_json`,
  turns a mid-stream error into an error event, and appends `[DONE]`.
- `with_routing_headers` adds the `x-synapse-*` headers from the plan's `RoutingReport`,
  defined in `routing/jev_router.rs`, and `with_guard_header` adds `x-synapse-guard` when input
  or buffered output was masked or blocked.
- A `GatewayError` from any step becomes the JSON error body and status in
  [`error.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/error.rs).

## 8. Usage accounting

Usage is recorded for requests that produced a response, as well as input guardrail blocks and
Jev classifier checks (`lane = "guard"`).

- **Buffered:** `Gateway::record` in `gateway.rs` prices the completion with
  [`pricing.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/pricing.rs),
  enqueues a ledger row, and emits the request metrics through `GenAiSpan::emit_metrics` in
  [`observability.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/observability.rs),
  before the response is returned.
- **Streaming:** the `GuardedStream` returned by `chat_stream` wraps the committed stream in a
  `StreamSideEffects` guard. The guard reads the token counts from the final stream item and
  does the same work in its `Drop`, so it runs once however the stream ends: completion, error
  or client disconnect. A stream that failed mid-way is recorded with status `error`.

The row goes to `LedgerHandle::enqueue` in
[`ledger/mod.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/ledger/mod.rs),
which never waits: it puts the row on a bounded queue of 10,000, or drops it and counts
`synapse_ledger_dropped_total`. A background task writes each row to the sink, or to every sink
through `FanoutLedger`.
[`ledger/connect.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/ledger/connect.rs)
connects the sinks at startup, and each sink has its own file: `sqlite.rs`, `postgres.rs`,
`pubsub.rs` and `sns.rs`. The row format is in
[`ledger/event.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/ledger/event.rs).

The metrics are the instruments in
[`telemetry.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/telemetry.rs),
which also builds the Prometheus and OTLP exporters. See the
[metrics catalogue](../reference/metrics-catalogue.md).

## Other endpoints

The other endpoints take shorter paths through the same components:

- **Embeddings.** `Gateway::embed_contextual` in `gateway.rs` looks up the alias in
  [`routing/embeddings.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/routing/embeddings.rs),
  screens `input` items under the alias's guardrail policy (`Gateway::guard_lane` with
  `GuardLane::Embeddings`), walks its legs through the embedders in
  [`embeddings/`](https://github.com/sustentabilitas/synapse-gateway/tree/main/crates/synapse-gateway/src/embeddings),
  and records usage.
- **Gemini passthrough.** `gemini_passthrough` in `server.rs` screens content actions with
  `screen_gemini` (`GuardLane::Passthrough`), forwards the body with
  `VertexNativeProvider::passthrough_request`, picks fallback legs with
  `RouteTable::vertex_fallback_chain`, and meters usage with a drop guard like the streaming
  one.
- **Jev passthrough.** `jev_passthrough` in `server.rs` screens `state` and `questions` string
  leaves under the `default` policy (`GuardLane::Jev`), forwards the body through
  `jev_native.rs`, and meters it the same way.
- **A2A.** The registry endpoints come from the `synapse-a2a` crate, merged into the router in
  `main.rs`. See [Workspace crates](./workspace-crates.md#synapse-a2a).
