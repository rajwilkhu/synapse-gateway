---
sidebar_position: 1
title: HTTP API
description: Every endpoint the gateway binary serves, with its request and response formats, the x-synapse-* headers and the error codes.
---

The gateway binary serves three listeners:

- the **API**, on `SYNAPSE_ADDR` (default `0.0.0.0:8080`), with every endpoint on this page
  except metrics and the A2A admin endpoints;
- the **metrics** endpoint, on `SYNAPSE_METRICS_ADDR` (default `0.0.0.0:9090`);
- the **A2A admin** endpoints, `/internal/a2a/*`, on `SYNAPSE_ADMIN_ADDR` (default
  `127.0.0.1:8081`), or on the API with `shared`.

All speak plain HTTP and none authenticates callers; see
[Security](../operating/security.md).

## Endpoints

| Method | Path | Purpose |
|---|---|---|
| `GET` | `/health` | Liveness check. |
| `GET` | `/v1/models` | List the chat route aliases. |
| `POST` | `/v1/chat/completions` | OpenAI-compatible chat completions, streaming or not. |
| `POST` | `/v1/embeddings` | OpenAI-compatible embeddings. |
| `POST` | `/v1beta/models/{model}:{action}` | Gemini-native passthrough to Vertex AI. |
| `POST` | `/v1/models/{model}:{action}` | Gemini-native passthrough to Vertex AI. |
| `POST` | `/google/models/{model}:{action}` | Gemini-native passthrough to Vertex AI. |
| `POST` | `/typesafe/v1/systemone` | Jev passthrough to TypeSafe System One. |
| `POST` | `/internal/a2a/agents` | Register an A2A agent (admin listener, `SYNAPSE_ADMIN_ADDR`). |
| `DELETE` | `/internal/a2a/agents/{id}` | Remove an A2A agent (admin listener, `SYNAPSE_ADMIN_ADDR`). |
| `GET` | `/.well-known/a2a-agent-catalog.json` | List the registered A2A agents. |
| `GET` | `/a2a/agents/{id}/.well-known/agent-card.json` | One agent's A2A card. |
| `GET` | `/a2a/agents/{id}/resolve` | One agent's endpoint and card. |
| `GET` | `/metrics` and `/` (metrics port) | Prometheus metrics. |

Any other path returns `404` with an empty body, and a known path with the wrong method returns
`405`.

## Request headers

Every endpoint except health, models, A2A and metrics reads these optional attribution headers
and records them on the request's [cost ledger](../guides/cost-ledger.md) rows.
[Tenant attribution](../guides/tenant-attribution.md#attribution-headers) explains each one.

| Header | Effect |
|---|---|
| `x-synapse-tenant` | Tenant. Without it, `SYNAPSE_DEFAULT_TENANT` (default `unattributed`). |
| `x-synapse-workspace` | Workspace within the tenant. |
| `x-synapse-user` | End user. |
| `x-synapse-thread` | Conversation or agent thread. |
| `x-synapse-message` | Message id. On chat completions and the passthroughs it also becomes the request id. |
| `x-synapse-user-task-type` | Your own label for the work, recorded as sent. |
| `x-synapse-ai-task-type` | Overrides the [AI task type](../guides/tenant-attribution.md#ai-task-types). |

## Response headers

Chat completion responses carry the routing report.
[Jev router](../guides/jev-router.md#response-headers) describes when each header appears.

| Header | Values |
|---|---|
| `x-synapse-routing` | `static`, `jev` or `static-override`. Always present on chat completions. |
| `x-synapse-tier` | The tier whose leg served the request. |
| `x-synapse-tier-decided` | The tier Jev chose, when a different tier served. |
| `x-synapse-reasoning-effort` | The serving leg's effort, or `client` when the request set its own. |
| `x-synapse-routing-degraded` | `timeout`, `error`, `low_confidence` or `jev_unavailable`. |

## Errors

Gateway errors are JSON in the OpenAI shape:

```json
{
  "error": {
    "type": "model_not_found",
    "message": "unknown model alias 'chat-typo'",
    "code": "model_not_found"
  }
}
```

| Status | `code` | When |
|---|---|---|
| `400` | `invalid_request_error` | The request is invalid for its route, such as an unknown `routing_strategy`, a `jev` block on the wrong kind of route, a mismatched embedding `dimensions`, or a native Vertex `4xx` that stops the chain. Also returned by a passthrough whose provider isn't configured. |
| `400` | `native_feature_unsupported` | The request uses native Vertex features and the route has no `vertex` leg. |
| `400` | `content_blocked` | A guardrail policy blocked the request. `type` is `content_policy_violation` and a `scanners` array names the scanners. See [Block response](../configuration/guardrails-policy.md#block-response). |
| `404` | `model_not_found` | The `model` is not a chat route alias (chat) or an embedding alias (embeddings). |
| `502` | `all_legs_failed` | Every leg failed. A `failures` array lists each leg's `provider`, `model` and `message`. See [When every leg fails](../guides/fallback-chains.md#when-every-leg-fails). |
| `502` | `upstream_error` | A provider failed where no further fallback was possible, such as the last native Vertex leg or an embedding leg. |

The `message` is human-readable and can change between versions; match on `code`. A body that
isn't valid JSON, or doesn't match the endpoint's schema, is rejected before it reaches the
gateway, with a plain-text message instead of this shape: `400` for malformed JSON, `415`
without a `Content-Type: application/json` header, and `422` for a missing or mistyped field.
Bodies larger than 2 MB are rejected with `413`.

The passthrough endpoints return the provider's status and body unchanged when the provider
answers. See [Gemini passthrough](#gemini-passthrough) and [Jev passthrough](#jev-passthrough).

## `GET /health`

Returns `200` with the body `ok`. It doesn't check providers or the ledger; it only shows that
the process is serving HTTP.

## `GET /v1/models`

Lists the chat route aliases from `routes.toml`, sorted by name:

```json
{
  "object": "list",
  "data": [
    { "id": "auto", "object": "model", "owned_by": "synapse" },
    { "id": "chat", "object": "model", "owned_by": "synapse" }
  ]
}
```

Embedding aliases are not listed.

## `POST /v1/chat/completions`

An OpenAI chat completion request. `model` is a route alias; the route decides which providers
and models serve it.

| Field | Type | Description |
|---|---|---|
| `model` | string | Required. A route alias from `routes.toml`. |
| `messages` | array | Required. OpenAI messages: `role`, `content` (a string, an array of content parts, or `null` on an assistant tool-call turn), and optionally `tool_calls`, `tool_call_id` and `name`. |
| `stream` | boolean | `true` for server-sent events. See [Streaming](../guides/streaming-and-tools.md#streaming). |
| `temperature` | number | Sampling temperature. |
| `max_tokens` | integer | Output token limit. Honoured on the native Vertex lane only. |
| `response_format` | object | `{"type": "text" \| "json_object" \| "json_schema", "json_schema": {...}}`. Standard lane only; on the native Vertex lane use `vertex.response_schema`. |
| `tools` | array | OpenAI function tools. See [Tool calling](../guides/streaming-and-tools.md#tool-calling). |
| `tool_choice` | string or object | Honoured on the native Vertex lane only. |
| `routing_strategy` | string | `static` or `jev`. See [Override the decision per request](../guides/jev-router.md#override-the-decision-per-request). |
| `vertex` | object | Native Vertex features: `cached_content`, `media_uris`, `response_schema`, `thinking_config`. Selects the native Vertex lane; see [Lane detection](../overview/architecture.md#lane-detection) and the [native Vertex guide](../guides/native-vertex.md). |
| `jev` | object | Typed questions for the Jev lane: `questions`, optional `state`, optional `extract`. See the [Jev lane guide](../guides/jev-lane.md). |

The gateway accepts any other field without an error, but forwards very few of them:

- The **standard lane** sends the provider `messages`, `tools`, `temperature`,
  `response_format` and an OpenAI `reasoning_effort`. It drops `max_tokens`, `tool_choice` and
  every other field, such as `top_p`, `stop` or `seed`.
- The **native Vertex lane** sends `messages`, `tools`, `tool_choice`, `temperature`,
  `max_tokens` and the `vertex` block. See
  [Other request fields](../guides/native-vertex.md#other-request-fields).

[Limitations](./limitations-roadmap.md#request-fields) lists the gaps.

### Response

A non-streaming request returns a `chat.completion`:

```json
{
  "id": "chatcmpl-3f1c9a52-7d4e-4b8a-9c1e-2a6b0f9d8e71",
  "object": "chat.completion",
  "created": 1790620000,
  "model": "gemini-3.5-flash-lite",
  "choices": [{
    "index": 0,
    "message": { "role": "assistant", "content": "Hello!" },
    "finish_reason": "stop"
  }],
  "usage": { "prompt_tokens": 9, "completion_tokens": 3, "total_tokens": 12 }
}
```

- `id` is `chatcmpl-` followed by the request id: the `x-synapse-message` header, or a
  generated UUID.
- `model` is the model of the leg that served the request, not the route alias.
- `finish_reason` is `stop`, `length` or `tool_calls`. A tool call has `content: null` and a
  `tool_calls` array.
- A [hybrid extraction](../guides/jev-lane.md#hybrid-extraction) response adds a `jev` object
  with `answers`, `survivors` and `degraded`.

### Streaming response

With `"stream": true`, the response is `text/event-stream`. Each event is a
`chat.completion.chunk` on a `data:` line, and the stream ends with `data: [DONE]`:

```text
data: {"id":"chatcmpl-...","object":"chat.completion.chunk","created":0,"model":"gemini-3.5-flash-lite","choices":[{"index":0,"delta":{"content":"Hel"},"finish_reason":null}]}

data: {"id":"chatcmpl-...","object":"chat.completion.chunk","created":0,"model":"gemini-3.5-flash-lite","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}

data: [DONE]
```

Chunks carry no `usage` and `created` is always `0`. A failure after the first chunk arrives as
an event whose body is an error object, followed by `[DONE]`; see
[Fallback while streaming](../guides/streaming-and-tools.md#fallback-while-streaming).

## `POST /v1/embeddings`

| Field | Type | Description |
|---|---|---|
| `model` | string | Required. An embedding alias from the `embeddings` table of `routes.toml`. |
| `input` | string or array of strings | Required. The texts to embed. |
| `dimensions` | integer | Optional. Must equal the alias's `dimensions`. |

The response is an OpenAI `list` of `embedding` objects, in input order, with
`usage.prompt_tokens` and `usage.total_tokens`. `model` in the response is the alias. See
[Send a request](../guides/embeddings.md#send-a-request).

## Gemini passthrough

`POST /v1beta/models/{model}:{action}`, `POST /v1/models/{model}:{action}` and
`POST /google/models/{model}:{action}` behave identically; the three prefixes match the API
versions Google's Gemini SDKs use. The body is a Gemini request, forwarded verbatim to Vertex
AI with the gateway's credentials.

- `{model}` is a route alias or a Vertex AI model name, and `{action}` is the part after the
  last colon, such as `generateContent`, `streamGenerateContent` or `countTokens`.
- `streamGenerateContent` with `?alt=sse` streams the response through.
- `generateContent` and `streamGenerateContent` write a ledger row with lane `passthrough`. On
  a `5xx`, `429` or `408` response or a connection error, they try the following `vertex` legs
  of the route that lists the model; see
  [Gemini SDK clients](../guides/native-vertex.md#gemini-sdk-clients).
- Other actions are forwarded once, without a ledger row.
- A model that no route lists is forwarded as a single attempt only with
  `SYNAPSE_PASSTHROUGH_MODELS=any`; by default it gets `404`.
- Streamed calls (`?alt=sse`) can run for up to an hour. Every other call is bounded by
  `SYNAPSE_REQUEST_TIMEOUT_SECS`.
- Vertex AI's status and body are returned unchanged. A connection error on the last attempt
  returns `502` with code `upstream_error`. If the gateway has no Vertex AI project configured,
  the endpoint returns `400`.

Generation requests are scanned under the matched route's policy, else `default` (see [What is
scanned](../configuration/guardrails-policy.md#what-is-scanned)). With
`SYNAPSE_PASSTHROUGH_MODELS=routed` (the default), a model that is neither a route alias nor a
`vertex` leg gets `404`. The gateway's own errors on this endpoint use Google's error shape.

## Jev passthrough

`POST /typesafe/v1/systemone` forwards a TypeSafe System One `{state, questions}` body verbatim
with the gateway's `TYPESAFE_API_KEY`. The body must be a JSON object; if it has no `model`,
the gateway sets `jev-latest`. TypeSafe's status and body are returned unchanged, and usage is
recorded in the ledger with lane `passthrough`. Without `TYPESAFE_API_KEY`, the endpoint
returns `400`. Requests are scanned under the `default` policy. See [Jev
passthrough](../guides/jev-lane.md#jev-passthrough).

## A2A agent registry

The gateway binary serves an in-memory registry of A2A agents, seeded at startup from
`a2a.toml` (see [Configuration keys](./configuration-keys.md#a2atoml)). Each gateway instance
has its own registry. The admin endpoints are served on `SYNAPSE_ADMIN_ADDR` (default
`127.0.0.1:8081`), not on the API port, unless it is set to `shared`.

### `POST /internal/a2a/agents`

Registers an agent. Every field except `ttl_seconds` is required:

```json
{
  "id": "invoice-agent",
  "name": "Invoice agent",
  "description": "Extracts line items from invoices",
  "endpoint_url": "https://agents.example.com/invoice",
  "card_url": "https://agents.example.com/invoice/.well-known/agent-card.json",
  "tags": ["finance"],
  "card": { "name": "Invoice agent", "skills": [] },
  "ttl_seconds": 3600
}
```

Returns `204`. If the id is already registered, the existing entry is kept and the request
still returns `204`. With `ttl_seconds`, the agent disappears from the catalogue that many
seconds after registration.

### `DELETE /internal/a2a/agents/{id}`

Removes the agent. Returns `204`, whether or not the id existed.

:::warning
The `/internal/` endpoints have no authentication. Block them at your proxy; see
[The A2A admin endpoints are open](../operating/security.md#the-a2a-admin-endpoints-are-open).
:::

### `GET /.well-known/a2a-agent-catalog.json`

```json
{
  "version": "1.0",
  "agents": [{
    "id": "invoice-agent",
    "name": "Invoice agent",
    "description": "Extracts line items from invoices",
    "card_url": "https://agents.example.com/invoice/.well-known/agent-card.json",
    "endpoint_url": "https://agents.example.com/invoice",
    "tags": ["finance"]
  }]
}
```

### `GET /a2a/agents/{id}/.well-known/agent-card.json`

Returns the agent's card as registered, or `404` with an empty body.

### `GET /a2a/agents/{id}/resolve`

Returns `{"id", "endpoint_url", "card_url", "card"}` for the agent, or `404` with an empty
body.

## Metrics

`GET /metrics` and `GET /` on the metrics port return the Prometheus text format. See
[Metrics](../operating/metrics.md) and the [metrics catalogue](./metrics-catalogue.md).
