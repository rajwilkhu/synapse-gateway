---
sidebar_position: 5
title: Fallback chains
description: How Synapse walks a route's legs when a provider fails, which failures move to the next leg on each lane, and how to design chains that fail over well.
---

Use a fallback chain whenever a single provider or region going down, throttling you or
timing out should not become an error for your users. A route lists several legs, and
Synapse tries them in order until one answers; the client sends one request and gets one
response, without knowing a leg failed. Chains also let you put a cheaper or faster model
first and keep a stronger one in reserve.

The keys are described in [Routes](../configuration/routes.md), and the
[fallback tutorial](../get-started/tutorials/fallback-across-providers.md) shows a chain
failing over from Vertex AI to OpenAI.

## The order of legs

- On a static route, the chain is the route's `legs`, in order.
- On a `strategy = "jev"` route, the chain starts with the chosen tier's legs, then each
  harder tier's, then each easier tier's. See [Jev router](jev-router.md#tier-fallback).
- Under lenient provider validation, legs whose provider cannot be built are removed at
  startup, and the chain is what remains. See
  [Providers](../configuration/providers.md#strict-and-lenient-validation).

Only one leg serves each request. Synapse never sends the same request to two legs at once.
On the **standard lane**, each `(provider, model)` leg has its own circuit breaker and
retry policy (up to 3 total attempts with exponential backoff for transient errors before
failing over to the next leg, and skipping legs whose circuit breaker is open). On the
native Vertex, Jev and embedding lanes, each leg gets one attempt per request. See
[Limitations and roadmap](../reference/limitations-roadmap.md#resilience).

## What moves to the next leg

What counts as a failure depends on the lane serving the request and, on the standard lane,
on whether the client streams.

### Standard lane

For a **non-streaming** request, any failure moves to the next leg:

- an error response from the provider, including `4xx` errors such as an unknown model or an
  invalid API key;
- no first chunk within `SYNAPSE_REQUEST_TIMEOUT_SECS`;
- a gap longer than `SYNAPSE_STREAM_IDLE_TIMEOUT_SECS` between chunks;
- a stream that breaks or ends before the model finishes.

Synapse buffers each leg's whole response before sending anything, so even a leg that fails
half-way through is replaced cleanly by the next one.

For a **streaming** request, the same failures move to the next leg only until the first
chunk arrives. Synapse then commits to that leg and forwards chunks as they come; a later
failure ends the stream with an `error` event. See
[Streaming and tool calling](streaming-and-tools.md#fallback-while-streaming).

### Native Vertex lane

Requests with [native Vertex features](native-vertex.md) use only the route's `vertex`
legs, and are stricter about what they retry:

- A `5xx`, `429` or `408` response, a connection error or a timeout while opening the
  stream moves to the next `vertex` leg.
- Any other `4xx` stops the chain and returns `400` with Vertex's message. These errors,
  such as an invalid schema or an expired cache, would fail on every leg.
- Once Vertex AI accepts the request, Synapse is committed to that leg, for streaming and
  non-streaming clients alike. A failure after that point fails the request with `502` and
  error code `upstream_error`.

### Jev lane

Requests with a `jev` block try the route's `typesafe` legs first. A `429`, `408`, `5xx` or
connection error moves to the next one; any other error stops the request with `400`. When
every `typesafe` leg fails retryably, the route's other legs answer as a normal chat
completion. See [Jev lane](jev-lane.md#when-jev-fails).

### Embeddings

Embedding aliases try their legs in order. Any error moves to the next leg, and a leg whose
provider is not configured is skipped. See [Embeddings](embeddings.md#fallback).

## When every leg fails

| Lane | Response |
|---|---|
| Standard | `502` with error code `all_legs_failed` and a `failures` array naming each leg's provider, model and error message, or `503` with `all_circuits_open` when every leg's circuit breaker is open. |
| Native Vertex | `502` with error code `upstream_error` and the last leg's error. |
| Jev, no other legs | `502` with error code `all_legs_failed`. |

```json
{
  "error": {
    "type": "all_legs_failed",
    "code": "all_legs_failed",
    "message": "all legs of route 'chat' failed",
    "failures": [
      { "provider": "vertex", "model": "gemini-3.5-flash-lite", "message": "..." },
      { "provider": "openai", "model": "gpt-4o-mini", "message": "..." }
    ]
  }
}
```

## What gets recorded

Only the leg that served the request is recorded: the ledger row, the response's `model`
field and the `model` label on `synapse_requests_total` all name it. Failed attempts are not
recorded, so tokens a provider consumed on a leg that broke mid-way do not appear in the
ledger. See [Cost ledger](cost-ledger.md#accuracy).

## Design a chain

- **Mix providers or regions.** Two legs on the same provider and region usually fail
  together. Put a different provider, or the same model in another region, behind the
  first leg.
- **Put the leg you want to pay for first.** Later legs only run when earlier ones fail, so
  order by preference, not by strength.
- **Keep native requests in mind.** A native Vertex request can only use `vertex` legs, so a
  route that serves them needs more than one `vertex` leg to fail over at all.
- **Test every leg on its own.** On the standard lane a misconfigured leg, such as a
  misspelled model name, fails over silently on every request, adding latency and hiding the
  problem. After changing models or credentials, call each leg through a single-leg route.
- **Size timeouts for the chain.** Each leg can use up to `SYNAPSE_REQUEST_TIMEOUT_SECS`
  before the next one starts, so a three-leg chain can take three times as long to fail.
