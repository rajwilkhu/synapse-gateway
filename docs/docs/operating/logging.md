---
sidebar_position: 2
title: Logging
description: What the Synapse gateway logs, in what format, and how to filter it with RUST_LOG.
---

Read the gateway's logs to confirm what it loaded at startup, to see why a ledger sink or
Jev routing decision failed, and to spot configuration problems. The logs are for operators:
they record the gateway's own events, not a line per request, and never prompts or model
output.

## Format

The gateway writes one line per event to standard output, using the
[`tracing`](https://docs.rs/tracing) crate's plain-text format: a UTC timestamp, the level,
the target (the Rust module that logged it), the message and any fields.

```text
2026-09-28T20:12:50.594884Z  INFO synapse_gateway: synapse-gateway metrics listening addr=0.0.0.0:9090 otlp=false
2026-09-28T20:12:50.771039Z  INFO synapse::ledger::connect: ledger sink connected backend="postgres"
2026-09-28T20:12:50.779484Z  INFO synapse_gateway: synapse-gateway listening addr=0.0.0.0:8080
```

Lines include ANSI colour codes unless the `NO_COLOR` environment variable is set to a
non-empty value. Set `NO_COLOR=1` in containers so your log collector receives plain text.
There is no JSON output format.

## Filter with `RUST_LOG`

`RUST_LOG` sets which events are written, using `tracing`'s
[filter syntax](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html).
The default is `info`. An invalid value also means `info`.

| Target | What logs under it |
|---|---|
| `synapse_gateway` | The binary: startup and listener messages. |
| `synapse` | The gateway library, such as `synapse::ledger::connect`, `synapse::resilience` and `synapse::routing::table`. |
| `synapse::guard` | Guardrail non-pass outcomes (`block`, `observe`, `masked`, `flag`) and Jev classifier failures. |
| `synapse::routing` | Jev router decisions and route leg fallbacks. |
| `synapse_a2a` | The A2A registry seed. |
| Others | Dependencies, such as `sqlx`. |

Examples:

```bash
RUST_LOG=info                     # the default
RUST_LOG=warn,synapse=info        # only warnings from dependencies
RUST_LOG=info,sqlx=warn           # hide Postgres notices at startup
RUST_LOG=info,synapse::routing=warn   # drop the per-request "route planned" events
RUST_LOG=info,synapse::guard=warn     # only log guardrail block and observe (dry-run) events
```

With the Postgres ledger, `sqlx` logs an `info` notice for each column the gateway checks
when it creates the `usage_events` table, such as
`column "user_id" of relation "usage_events" already exists, skipping`. They are harmless;
`sqlx=warn` hides them.

## What is logged

### At startup

| Level | Message | Meaning |
|---|---|---|
| `INFO` | `synapse-gateway metrics listening` | The metrics listener is up; `otlp` says whether OTLP push is on. |
| `INFO` | `ledger sink connected` | A ledger sink connected; `backend` names it. |
| `ERROR` | `ledger sink connect failed; skipping` | A ledger sink could not connect and records nothing until the gateway restarts; `error` says why. |
| `WARN` | `no ledger sinks available; usage accounting disabled` | No sink connected. The gateway serves requests without recording usage. |
| `WARN` | `retrying pubsub ledger connect` | The Pub/Sub sink is retrying its connection. |
| `WARN` | `dropping route legs: ...` | Lenient provider validation removed a provider's legs because its credential is missing. |
| `WARN` | `lenient provider validation pruned the route table` | Summary of what lenient validation removed, with alias counts before and after. |
| `WARN` | `jev route downgraded to static ...` | A `jev` route lost its Jev router under lenient validation. |
| `WARN` | `jev route default_tier pruned; re-picked` | A `jev` route's `default_tier` lost all its legs, and another tier took its place. |
| `INFO` | `a2a seed file absent; starting with empty A2A registry` | No `a2a.toml` was found. |
| `INFO` / `WARN` | `seeded a2a agent`, `skipping a2a seed agent: card fetch failed after retries` | A2A seed results, one per agent. |
| `INFO` | `synapse-gateway listening` | The API listener is up and the gateway is ready. |

If the gateway cannot start, for example because `routes.toml` is missing or invalid or a
provider credential is unset under strict validation, it prints `Error:` and the cause to
standard error and exits with a non-zero status.

### While serving

| Level | Target | Message | Meaning |
|---|---|---|---|
| `WARN` | `synapse::guard` | `guardrail match` | A guardrail scan produced `outcome = "block"` or `outcome = "observe"` (dry-run would-block). Fields: `policy`, `lane`, `direction`, `outcome`, `scanners`. See [Guardrails policy](../configuration/guardrails-policy.md#logs-and-ledger-rows). |
| `INFO` | `synapse::guard` | `guardrail match` | A guardrail scan produced `outcome = "masked"` or `outcome = "flag"` (`action = "log"` / `warn` severity). |
| `WARN` | `synapse::guard` | `jev guard classifier failed after trying all typesafe legs` | A policy's Jev classifier gave no valid answer; `route`, `model`, `error.kind` and `http.status` say how. |
| `INFO` | `synapse::routing` | `route planned` | One per request on a `jev` route, with the mode, decided tier, outcome, effort and Jev's scores. See [Jev router](../guides/jev-router.md#metrics-and-logs). |
| `WARN` | `synapse::routing` | `jev decision timed out; routing to default_tier` | Jev did not answer within `timeout_ms`. |
| `WARN` | `synapse::routing` | `jev decision failed; routing to default_tier` | Jev failed; `error.kind` and `http.status` say how. The response body is never logged. |
| `WARN` | `synapse::routing` | `leg failed; falling back to next leg` | A route leg failed and the chain advanced to the next leg, with `route`, failing `provider`/`model`, next `provider`/`model`, and error classification. See [Fallback chains](../guides/fallback-chains.md). |
| `WARN` | `synapse::resilience` | `retrying outbound call` | A transient provider error (`5xx`, `429`, `408`, timeout/connect) is being retried on the same leg with exponential backoff (`label`, `attempt`, `next_delay_ms`, `error`). |
| `WARN` / `INFO` | `synapse::resilience` | `circuit breaker opened`, `circuit breaker half-open`, `circuit breaker closed` | A provider's circuit breaker changed state (`name`, `consecutive_failures`). |
| `WARN` | `synapse::ledger` | `ledger write failed` | The only ledger sink rejected a row, which is lost. |
| `WARN` | `synapse::ledger` | `ledger sink write failed` | One of several sinks rejected a row; `backend` names it. The other sinks still got the row. |
| `WARN` | `synapse::ledger` | `ledger background writer stopped` | The ledger writer exited; no further rows are recorded. |
| `ERROR` | `synapse_gateway` | `metrics server stopped` | The metrics listener failed. |

Ledger failure lines carry the row's `tenant`, and `ledger write failed` also its
`request_id`, so you can find the affected usage.

### Not logged

- **Requests.** There is no access log. Use the access logs of the proxy or load balancer in
  front of the gateway.
- **Content.** Prompts, messages, upstream error bodies and model output are never logged.

## Tracing

The gateway emits logs only. It does not create OpenTelemetry trace spans or export traces,
so a request through Synapse does not appear in a distributed trace, and `RUST_LOG` only
controls logs. For request-level telemetry, use the [metrics](metrics.md) and the
[cost ledger](../guides/cost-ledger.md), whose `request_id` you can set per request with the
`x-synapse-message` header; see
[Correlate requests](../guides/tenant-attribution.md#correlate-requests). Tracing is listed
under [Limitations and roadmap](../reference/limitations-roadmap.md#observability).
