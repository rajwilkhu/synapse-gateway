---
sidebar_position: 2
title: Architecture
description: How Synapse moves a chat completion request through guardrails, route planning, one of three lanes and a fallback chain of providers.
---

Synapse accepts OpenAI-compatible chat completion requests and serves each one through one
of three backend lanes: the **standard** lane, the **native Vertex** lane or the **Jev**
lane. The request body alone decides the lane, so a client opts into Vertex-only features
or Jev decisions by adding an extension block, with no separate endpoint.

## Request flow

```text
client
  │  POST /v1/chat/completions   (model = route alias, x-synapse-tenant header)
  ▼
route lookup ─────────────► 404 model_not_found (unknown alias)
  │
  ▼
input guardrails (route policy; enforced in `block` mode or dry-run in `observe` mode)
  ├─► 1. in-process span scanners (`llm-guard`: block, `[REDACTED:...]` mask, or log)
  └─► 2. Jev semantic classifier (optional `noul` questions on post-mask input)
  │                        └─► 400 content_blocked (or 503 guard_unavailable)
  ▼
route planning             static route: its legs
  │                        strategy = "jev": the Jev router scores difficulty + reasoning
  ▼
lane detection
  ├─► standard lane        (genai: OpenAI, Qwen, oai_compat, Vertex; per-leg retries & breaker)
  ├─► native Vertex lane   (Vertex REST: :streamGenerateContent)
  └─► Jev lane             (TypeSafe System One questions or `jev.extract` hybrid extraction)
  │
  ▼
fallback chain             leg 1 → leg 2 → … until one succeeds
  │
  ▼
output guardrails          optional `output_scanners` (buffered or streaming holdback window)
  │
  ▼
response to the client (JSON or server-sent events)
  │
  ├─► cost ledger          tokens and cost per tenant (`standard`/`native`/`jev`/`guard`)
  └─► metrics & logs       OpenTelemetry `synapse_*` metrics + `synapse::*` tracing events
```

Each route alias in `routes.toml` maps to an ordered list of legs (provider plus model).
Synapse tries the legs in order until one succeeds. On the standard lane, any failure moves
to the next leg: an error response, a first-chunk timeout or a broken stream. On the native
Vertex lane, only a `5xx`, `429` or `408` response, a connection error or a timeout moves
on; any other `4xx` stops the chain. A streaming response can fall back only until its
first chunk reaches the client. See the [fallback chains guide](../guides/fallback-chains.md).

The ledger write never blocks the response: if the ledger's queue is full, the event is
dropped and counted in `synapse_ledger_dropped_total`. See the
[cost ledger guide](../guides/cost-ledger.md) and the
[metrics catalogue](../reference/metrics-catalogue.md). For a step-by-step walk through the
source, see [Request pipeline](../internals/request-pipeline.md).

## Lanes

### Standard lane

Requests without lane triggers use the standard lane, which calls providers through the
[`genai`](https://crates.io/crates/genai) crate. Any provider with an OpenAI-compatible API
can appear in the chain: OpenAI, Qwen (DashScope) and self-hosted vLLM, Ollama or TGI
through the `oai_compat` provider. Vertex legs work here too, without the native-only
features. See [Providers](../configuration/providers.md).

### Native Vertex lane

Requests that use Vertex-only features go to the native Vertex lane, which calls the Vertex
AI `:streamGenerateContent` REST endpoint directly, for streaming and non-streaming clients
alike; for a non-streaming client, Synapse buffers the stream into one response. The OpenAI
message format is translated to Vertex's, and these fields of the request's `vertex` block
are preserved:

- **`cached_content`**: a `cachedContents` resource name, for context caching.
- **`media_uris`**: Cloud Storage (`gs://`) URIs, attached as file parts with MIME type
  `video/mp4`.
- **`response_schema`**: a JSON schema sent as `generationConfig.responseSchema` for
  constrained decoding.
- **`thinking_config`**: passed through verbatim as `generationConfig.thinkingConfig`.

Only the route's `vertex` legs take part; other providers cannot serve these features. If
the route has no `vertex` leg, Synapse returns `400 Bad Request` with error code
`native_feature_unsupported` rather than silently dropping the features. See the
[native Vertex guide](../guides/native-vertex.md).

### Jev lane

Requests with a `jev` block carrying typed questions go to the route's `typesafe` legs.
TypeSafe System One (Jev) evaluates the questions against a state, which defaults to the
request's `messages`, and returns structured decisions as the message content. If every
`typesafe` leg fails with a retryable error, the route's remaining legs answer as a normal
chat completion, so clients must handle both response shapes. A route with `typesafe` legs
returns `400 Bad Request` to requests without a `jev` block. See the
[Jev lane guide](../guides/jev-lane.md).

## Lane detection

Synapse checks the request body in this order:

1. A `jev` block with a non-empty `questions` map selects the **Jev** lane. A `vertex` block
   on the same request still applies if the chain falls back to a native Vertex leg.
2. A `vertex` block with any of `cached_content`, `response_schema`, `thinking_config`, or a
   `media_uris` entry that starts with `gs://`, selects the **native Vertex** lane.
3. Anything else uses the **standard** lane.

For example, this request uses the native Vertex lane:

```json
{
  "model": "gemini-flash",
  "messages": [{ "role": "user", "content": "Summarise the video." }],
  "vertex": {
    "cached_content": "projects/my-gcp-project/locations/us/cachedContents/abc123",
    "media_uris": ["gs://my-bucket/video.mp4"],
    "response_schema": { "type": "object", "properties": { "summary": { "type": "string" } } }
  }
}
```

Lane detection and routing strategy are independent. On a `strategy = "jev"` route, the Jev
router chooses which tier's legs form the chain, and the lane still follows the request
body: a native Vertex request is served by the nearest tier that has a `vertex` leg. See the
[Jev router guide](../guides/jev-router.md).
