---
sidebar_position: 8
title: Cost ledger
description: What the Synapse cost ledger records for each request, where it writes it, how reliable the numbers are, and how to query spend per tenant.
---

Use the cost ledger to answer "who spent what, on which model, for which kind of work".
Synapse writes one row per completed request, with its tenant, route, serving model, token
counts and cost, into a database you own, and can publish the same events to a message bus
for billing or analytics pipelines. It is built for usage reporting and showback, not as an
exact invoice: read [Accuracy](#accuracy) before billing customers from it.

## What gets a row

| Event | `lane` | `provider` and `model` |
|---|---|---|
| A chat completion served on the standard lane | `standard` | The leg that served it. |
| A chat completion served on the native Vertex lane | `native` | The `vertex` leg that served it. |
| A Jev lane answer, or a hybrid extraction's Jev answer | `jev` | `typesafe` and the Jev build. |
| A Jev router decision | `jev` | `typesafe` and `jev_router.model`. |
| Each hybrid extraction | `standard` or `native` | The leg that served it. |
| An embedding request | `embedding` | The leg that served it; `output_tokens` is 0. |
| A Gemini `generateContent` or `streamGenerateContent` passthrough call | `passthrough` | `vertex` and the leg's model. |
| A Jev passthrough call | `passthrough` | `typesafe` and the model called. |
| An input guardrail scanner block (`op = "guard_input"`) | `guard` | `guard` and the triggered scanner name (e.g. `prompt_injection`); 0 tokens and `$0` cost. |
| A Jev guardrail classifier check (`op = "guard_classifier"`) | `guard` | `typesafe` and `classifier.model`. |

Failed chat and embedding requests write no row: a non-streaming request that fails, a
streaming request that fails before its first chunk, or a request rejected with `400`. Input
guardrail blocks and Jev classifier checks write a `lane = "guard"` row (`status = "blocked"`
when the request is blocked, or `status = "ok"` when the classifier allows the request or runs
in `observe` / `log` mode). A streaming response writes its row when the stream ends, whether it
completes, fails or the client disconnects; a stream that fails mid-way is recorded with
`status` `error`. Passthrough calls write a row even when they fail. Other Gemini
passthrough actions, such as `countTokens`, are forwarded without a row.

## Row format

The gateway creates the `usage_events` table on startup, in SQLite and in Postgres:

| Column | Description |
|---|---|
| `id` | Row id. |
| `ts` | When the request finished, in UTC. SQLite stores it as RFC 3339 text. |
| `tenant`, `workspace`, `user_id`, `thread_id`, `message_id` | Attribution from the request headers; see [Tenant attribution](tenant-attribution.md). |
| `route` | The route or embedding alias the client called. |
| `provider`, `model` | The leg that served the request, or `guard` / `typesafe` for guardrail rows. |
| `lane` | `standard`, `native`, `jev`, `embedding`, `passthrough` or `guard`. |
| `input_tokens`, `output_tokens` | Token counts reported by the provider. |
| `cost_usd` | Cost computed from `pricing.toml`. See [Pricing](../configuration/pricing.md#how-cost-is-computed). |
| `request_id` | The request id, shared by every row one request writes. |
| `status` | `ok`, `blocked` (for blocked guardrail checks), or `error` for failed streams and passthrough calls. |
| `user_task_type`, `ai_task_type` | Task classification; see [AI task types](tenant-attribution.md#ai-task-types). |

## Sinks

The ledger writes to one or more sinks, selected with `SYNAPSE_LEDGER_BACKENDS`:

- **`sqlite`**, the default, writes to a local file. Good for a single instance and for
  trying Synapse out.
- **`postgres`** writes to a shared database. Use it when several gateway instances should
  share one ledger.
- **`pubsub`** and **`sns`** publish each row as a JSON event to Google Cloud Pub/Sub or AWS
  SNS, for billing, analytics or data warehouse pipelines.

Every row goes to every configured sink, concurrently; one sink failing does not affect the
others. The Postgres, Pub/Sub and SNS sinks need their Cargo features, which the Docker
image includes. [Ledger](../configuration/environment-variables.md#ledger) in the
environment variables reference lists the connection settings for each sink.

A sink that cannot connect at startup is logged and skipped. If none connects, the gateway
still starts and serves requests, recording nothing, so check the startup logs after
configuring a sink.

## Delivery

Ledger writes never slow a request down. Synapse puts each row on an in-memory queue of
10,000 rows and a background task writes them out. That design has consequences:

- If the queue is full, new rows are dropped and counted in `synapse_ledger_dropped_total`.
- A failed write is logged and counted in `synapse_ledger_errors_total`, and not retried.
  With several sinks, the `backend` label names the failing sink; with one sink, it is
  `writer`.
- Rows still in the queue when the process stops are lost.

Alert on both counters being above zero. For usage you must not lose, publish to Pub/Sub or
SNS as well as a database, so a single sink's outage does not create a gap.

## Published events

The Pub/Sub and SNS sinks publish one JSON event per row, in camelCase, with the tenant as
`namespace`:

```json
{
  "namespace": "my-team",
  "workspace": "onboarding",
  "requestId": "0f8e5c1e-2d1b-4c7a-9a55-3f0c6f7d9b21",
  "timestamp": "2026-09-28T10:00:00Z",
  "type": "usage",
  "route": "chat",
  "provider": "vertex",
  "model": "gemini-3.5-flash-lite",
  "lane": "standard",
  "inputTokens": 128,
  "outputTokens": 256,
  "costUsd": 0.0006784,
  "status": "ok",
  "op": "chat",
  "aiTaskType": "conversation"
}
```

`workspace`, `user`, `threadId`, `messageId` and `userTaskType` appear only when set. `op`
says what produced the event: `chat` for a chat completion or a Gemini passthrough call,
`embedding`, `route_decision` for a Jev router decision, `guard_input` for an input guardrail
scanner block, `guard_classifier` for a Jev guardrail classifier check, or `systemone` for a
Jev passthrough call. The database tables have no `op` column.

Each message also carries attributes for subscription filters: `EventType`
(`Ledger.LLMTokensConsumed`), `namespace`, `requestId`, `type`, `provider` and `status`.
Pub/Sub messages use `requestId` as their ordering key.

## Accuracy

The ledger is only as accurate as the token counts and prices behind it. Known gaps:

- **Cached input on the native Vertex lane is priced at full rate.** `input_tokens` is Vertex
  AI's `promptTokenCount`, which includes tokens read from a context cache, and Synapse
  prices them all at the `input` price. Vertex AI bills cached tokens at a discount and
  charges separately for cache storage, so cached requests are overstated.
- **Thinking tokens on the native Vertex lane are not counted.** `output_tokens` is Vertex
  AI's `candidatesTokenCount`, which excludes thinking tokens, and Vertex AI bills thinking as
  output. Requests that think are understated. On the standard lane, Gemini's thinking tokens
  are included in `output_tokens`.
- **Failed attempts are not recorded.** Tokens a provider consumed on a leg that failed
  before another leg served the request do not appear.
- **Abandoned streams record zero tokens.** Providers report usage at the end of a stream, so
  a streaming request the client disconnects from early is recorded with 0 input and 0
  output tokens.
- **Unpriced chat models cost 0.** A chat model missing from `pricing.toml` is recorded with
  `cost_usd` 0. Embedding models fall back to a default price instead.
- **Dropped rows.** See [Delivery](#delivery).

Rows keep the raw token counts next to the cost, so you can recompute cost with your own
rates.

## Query the ledger

These queries use SQLite syntax. Spend per tenant per day:

```sql
SELECT tenant, substr(ts, 1, 10) AS day, SUM(cost_usd) AS usd,
       SUM(input_tokens) AS input_tokens, SUM(output_tokens) AS output_tokens
FROM usage_events
GROUP BY tenant, day
ORDER BY day, usd DESC;
```

Cost by kind of work and model:

```sql
SELECT ai_task_type, provider, model, COUNT(*) AS requests, SUM(cost_usd) AS usd
FROM usage_events
WHERE status = 'ok'
GROUP BY ai_task_type, provider, model
ORDER BY usd DESC;
```

The cost of a Jev-routed request, decision included:

```sql
SELECT request_id, SUM(cost_usd) AS usd, GROUP_CONCAT(model) AS models
FROM usage_events
WHERE route = 'auto'
GROUP BY request_id;
```
