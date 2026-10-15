---
sidebar_position: 3
title: Metrics catalogue
description: Every metric the gateway, synapse-proxy and synapse-mcp record, with its type, unit and labels.
---

This page lists every OpenTelemetry instrument in the workspace, with the name it has in
Prometheus. For how the gateway exports metrics, what the request metrics count and which
queries and alerts to start with, see [Metrics](../operating/metrics.md).

None of the instruments declare a unit in code. Durations are recorded in seconds, which the
`_seconds` suffix shows; counters count events or tokens.

## Gateway

The gateway serves these on its metrics port, `SYNAPSE_METRICS_ADDR` (default `0.0.0.0:9090`),
and pushes them over OTLP when `OTEL_EXPORTER_OTLP_ENDPOINT` is set. The Prometheus names are
exactly as listed. Histograms have the buckets 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1,
2.5, 5, 10, 30, 60 and 120 seconds; slower requests land only in the `+Inf` bucket.

### Chat requests

| Metric | Type | Unit | Labels |
|---|---|---|---|
| `synapse_requests_total` | Counter | requests | `route`, `model`, `system`, `lane` |
| `synapse_request_duration_seconds` | Histogram | seconds | `route`, `model`, `system`, `lane` |
| `synapse_input_tokens_total` | Counter | tokens | `route`, `model`, `system`, `lane` |
| `synapse_output_tokens_total` | Counter | tokens | `route`, `model`, `system`, `lane` |

- `route` is the route alias and `model` is the model of the leg that served the request.
- `system` is the serving leg's provider family: `vertexai` for `vertex`, `openai`, `dashscope`
  for `qwen`, and `oai_compat` for `oai_compat` and `typesafe`.
- `lane` is `standard`, `native` or `jev`. A streamed Jev answer is labelled `standard`.

Only requests that produced a response are counted; see
[What the request metrics count](../operating/metrics.md#what-the-request-metrics-count).

### Embeddings

| Metric | Type | Unit | Labels |
|---|---|---|---|
| `synapse_embeddings_total` | Counter | requests | `route`, `model`, `provider` |
| `synapse_embedding_duration_seconds` | Histogram | seconds | `route`, `model`, `provider` |

`route` is the embedding alias; `model` and `provider` name the leg that served it. Only
successful requests are counted, and the duration covers the serving leg alone. See
[Embeddings](../guides/embeddings.md#metrics).

### Passthrough

| Metric | Type | Unit | Labels |
|---|---|---|---|
| `synapse_passthrough_total` | Counter | calls | `provider`, `model`, `action`, `status` |
| `synapse_passthrough_fallback_total` | Counter | fallbacks | `from_model`, `to_model` |

- `synapse_passthrough_total` counts each attempt of the Gemini and Jev passthroughs.
  `provider` is `vertex` or `typesafe`; `action` is the Gemini action, or `systemone`; `status`
  is `ok` for a `2xx` response and `error` otherwise.
- `synapse_passthrough_fallback_total` counts each move to the next `vertex` leg in a Gemini
  passthrough.

### Jev

| Metric | Type | Unit | Labels |
|---|---|---|---|
| `synapse_routing_decisions_total` | Counter | requests | `route`, `tier`, `outcome` |
| `synapse_routing_decision_duration_seconds` | Histogram | seconds | `route` |
| `synapse_jev_extraction_total` | Counter | responses | `route`, `degraded` |

- `synapse_routing_decisions_total` counts requests to `strategy = "jev"` routes. `tier` is the
  tier the decision selected; `outcome` is `decided`, `low_confidence`, `timeout`, `error` or
  `static_override`. See [Jev router](../guides/jev-router.md#metrics-and-logs).
- `synapse_routing_decision_duration_seconds` times the call to Jev, so requests with
  `"routing_strategy": "static"` don't record it.
- `synapse_jev_extraction_total` counts hybrid extraction responses; `degraded` is `true` or
  `false`. See [Jev lane](../guides/jev-lane.md#ledger-and-metrics).

### Guardrails

| Metric | Type | Unit | Labels |
|---|---|---|---|
| `synapse_guard_scans_total` | Counter | scans | `lane`, `policy`, `outcome` |
| `synapse_guard_matches_total` | Counter | matches | `lane`, `policy`, `scanner`, `severity` |
| `synapse_guard_scan_duration_seconds` | Histogram | seconds | `lane`, `policy` |

`lane` is `chat`, `passthrough`, `embeddings` or `jev`, a different set of values from the
`lane` label on `synapse_requests_total` (`standard`, `native` or `jev`). `outcome` is `pass`,
`flag`, `block` or `observe`; `severity` is `block`, `warn` or `info`. See
[Guardrails policy](../configuration/guardrails-policy.md#metrics).

### Cost ledger

| Metric | Type | Unit | Labels |
|---|---|---|---|
| `synapse_ledger_dropped_total` | Counter | rows | none |
| `synapse_ledger_errors_total` | Counter | failed writes | `backend` |

- `synapse_ledger_dropped_total` counts rows dropped because the ledger queue was full.
- `synapse_ledger_errors_total` counts rows a sink failed to write. With one sink, `backend` is
  `writer`. With several, it is the failing sink's name: `sqlite`, `postgres`, `pubsub` or
  `sns`.

See [Delivery](../guides/cost-ledger.md#delivery).

### Resilience

| Metric | Type | Unit | Labels |
|---|---|---|---|
| `synapse_resilience_calls_total` | Counter | calls | `label`, `outcome` |
| `synapse_resilience_call_duration_seconds` | Histogram | seconds | `label`, `outcome` |
| `synapse_resilience_retry_attempts_total` | Counter | retries | `label` |
| `synapse_resilience_breaker_transitions_total` | Counter | transitions | `name`, `transition` |
| `synapse_resilience_breaker_state` | Gauge | state | `name`: 0 closed, 1 open, 2 half-open |

:::note
These instruments are defined, but the current gateway never records them: chat requests have
no retries or circuit breakers. They don't appear in the exposition.
:::

## synapse-proxy

The `synapse-proxy` binary serves these on its `metrics_addr` (default `0.0.0.0:9090`) at
`GET /metrics`, in Prometheus format only. An application that embeds the proxy library can add
an OTLP exporter with `Metrics::with_otlp`.

The proxy uses the OpenTelemetry Prometheus exporter's default settings, so its exposition
differs from the gateway's: counters get a second `_total` suffix, every series carries
`otel_scope_name="synapse-proxy"`, and a `target_info` series is present. The names below are
the ones Prometheus sees:

| Metric | Type | Unit | Labels |
|---|---|---|---|
| `synapse_proxy_requests_total_total` | Counter | requests | `route`, `method`, `status`, `outcome` |
| `synapse_proxy_request_duration_seconds` | Histogram | seconds | `route`, `method` |
| `synapse_proxy_upstream_retries_total_total` | Counter | retries | `route`, `reason` |
| `synapse_proxy_upstream_errors_total_total` | Counter | errors | `route`, `reason` |
| `synapse_proxy_transform_errors_total_total` | Counter | errors | `route`, `transform` |

- `route` is the proxy route's `name`, or `none` when no route matched. `status` is the HTTP
  status returned to the client.
- `outcome` is `forwarded`, `no_route`, `context_unbound`, `transform_rejected` or
  `upstream_error`. A request rejected for an oversized body isn't counted.
- The duration runs until the response headers are ready, not until a streamed body ends.
- `synapse_proxy_upstream_retries_total_total` counts retried sends; `reason` is `connect` or
  `send`.
- `synapse_proxy_upstream_errors_total_total` counts sends that failed after every retry;
  `reason` is always `send`.
- `synapse_proxy_transform_errors_total_total` counts rejections by a request or response
  transform; `transform` is `request` or `response`.

:::warning Histogram buckets
`synapse_proxy_request_duration_seconds` uses OpenTelemetry's default buckets, which are sized
for milliseconds (0, 5, 10, 25, 50, 75, 100, 250, 500, 750, 1000, 2500, 5000, 7500 and 10000),
while the values are in seconds. Every request faster than 5 seconds falls between the `le="0"`
and `le="5"` boundaries, so quantiles computed from it are meaningless below 5 seconds. Use
`_sum / _count` for average latency.
:::

## synapse-mcp

`synapse-mcp` records its instruments on a meter that the embedding application supplies; the
crate doesn't export anything itself.

| Instrument | Type | Unit | Labels |
|---|---|---|---|
| `broker_mcp_requests_total` | Counter | tool calls | `tool`, `upstream`, `outcome` |
| `broker_mcp_request_duration_seconds` | Histogram | seconds | `tool`, `upstream` |
| `broker_identity_injection_failures_total` | Counter | failures | `reason` |

- `tool` is the MCP tool name and `upstream` is the MCP server the call was routed to.
  `outcome` is `ok` or `error`.
- `broker_identity_injection_failures_total` counts calls refused because a required context
  key was missing; `reason` is always `missing_context_key`.

These are instrument names. When the meter comes from `synapse-proxy`'s `Metrics::meter()`, the
proxy's exporter settings apply: the counters appear as `broker_mcp_requests_total_total` and
`broker_identity_injection_failures_total_total`, the series carry
`otel_scope_name="sandbox-broker"`, and the histogram has the same millisecond-sized default
buckets as the proxy's.
