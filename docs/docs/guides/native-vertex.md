---
sidebar_position: 1
title: Native Vertex features
description: Use Vertex AI context caching, Cloud Storage media, strict response schemas, thinking configuration and native tool choice through Synapse's OpenAI-compatible API.
---

Use the native Vertex lane when a request needs something only Vertex AI can do: reuse a
context cache, read a video straight from Cloud Storage, force the answer into a JSON schema
with constrained decoding, or set Gemini's thinking configuration. You keep the OpenAI
request format and add a `vertex` block; Synapse sends the request to the Vertex REST API
directly instead of through the generic OpenAI-compatible adapter, which would drop those
fields. If a request needs none of this, leave the `vertex` block out and it rides the
standard lane, with fallback to any provider.

## How a request reaches the native lane

A request goes to the native Vertex lane when its `vertex` block has `cached_content`,
`response_schema` or `thinking_config`, or a `media_uris` entry that starts with `gs://`.
A `vertex` block with none of these, for example only `https://` media URIs, goes to the
standard lane, which ignores the block. [Lane detection](../overview/architecture.md#lane-detection)
has the full order, including how a `jev` block takes precedence.

On the native lane:

- Only the route's `vertex` legs take part. OpenAI, Qwen and `oai_compat` legs cannot
  express these features, so they are skipped rather than served a request with the
  features silently removed.
- A route without any `vertex` leg returns `400` with error code
  `native_feature_unsupported`.
- Synapse always calls `:streamGenerateContent`, and buffers the stream into one response
  for non-streaming clients.
- Each leg calls Vertex AI in its `region`, or in `VERTEX_LOCATION` when it has none. See
  [Regions](../configuration/routes.md#regions).

A `strategy = "jev"` route serves native requests from the nearest tier that has a `vertex`
leg; see [Jev router](jev-router.md#tier-fallback).

## Context caching

`cached_content` names a Vertex `cachedContents` resource. Synapse passes it as
`cachedContent`, so Gemini reads the cached content instead of you sending it again:

```json
{
  "model": "gemini-flash",
  "messages": [{ "role": "user", "content": "Which animals appear in the video?" }],
  "vertex": {
    "cached_content": "projects/my-gcp-project/locations/us/cachedContents/1234567890123456789"
  }
}
```

Synapse does not create or manage caches; create them with the Vertex AI API. A cache is
tied to one model and one location, so it must match the leg that serves the request. If
the cache has expired or belongs to another model or location, Vertex AI rejects the request
and Synapse returns `400` with Vertex's message. With several `vertex` legs on different
models, only the leg that matches the cache can succeed; the others fail with `400` and stop
the chain, so give cached requests a route whose first `vertex` leg matches the cache.

The [caching tutorial](../get-started/tutorials/native-vertex-caching.md) walks through
creating a cache and querying it.

:::warning
The ledger prices cached input tokens at the full `input` price, while Vertex AI bills them
at a discount. See [Cost ledger](cost-ledger.md#accuracy).
:::

## Cloud Storage media

`media_uris` lists Cloud Storage objects for Gemini to read. Synapse attaches each URI to the
last user message as a file part with MIME type `video/mp4`:

```json
{
  "model": "gemini-flash",
  "messages": [{ "role": "user", "content": "Describe this video." }],
  "vertex": { "media_uris": ["gs://cloud-samples-data/video/animals.mp4"] }
}
```

Because every URI is labelled `video/mp4`, use `media_uris` for video. Send images inline
instead, as `image_url` content parts with a base64 `data:` URL, which the standard and native
Vertex lanes both support.

## Structured output

`response_schema` is a JSON schema that constrains Gemini's output. Synapse sends it as
`generationConfig.responseSchema` with `responseMimeType` set to `application/json`, so
Vertex AI uses constrained decoding and the message content is a JSON string that matches
the schema:

```json
{
  "model": "gemini-flash",
  "messages": [{ "role": "user", "content": "List three primary colours." }],
  "vertex": {
    "response_schema": {
      "type": "object",
      "properties": { "colours": { "type": "array", "items": { "type": "string" } } },
      "required": ["colours"]
    }
  }
}
```

Synapse drops Gemini's thinking parts from the content, so the string is only the JSON
document. Vertex AI accepts a subset of JSON Schema; check Google's structured output
documentation for what `responseSchema` supports.

The OpenAI `response_format` field is not used on the native lane. On the standard lane,
`response_format` with `json_schema` or `json_object` is passed to the provider through the
`genai` crate instead.

## Thinking configuration

`thinking_config` is copied verbatim into `generationConfig.thinkingConfig`, so you can use
whatever the model accepts, for example `{ "thinkingLevel": "low" }` for Gemini 3 or
`{ "thinkingBudget": 2048 }` for Gemini 2.5.

The native lane ignores the OpenAI `reasoning_effort` field. On a `strategy = "jev"` route,
a tier's `effort` becomes a `thinkingBudget` on native legs, unless the request carries its
own `thinking_config`, which always wins. See [Effort](jev-router.md#effort).

## Tool calling

`tools` in OpenAI format become Vertex `functionDeclarations`, and `tool_choice` is honoured
through `toolConfig.functionCallingConfig`:

| `tool_choice` | Vertex `mode` |
|---|---|
| `"auto"` | `AUTO` |
| `"none"` | `NONE` |
| `"required"` | `ANY` |
| `{ "type": "function", "function": { "name": "..." } }` | `ANY` |

Naming one function sets mode `ANY`, which makes the model call a function but does not
restrict it to the one you named.

Tool calls in the response get the ids `call_0`, `call_1` and so on. When you send a tool
result back, Synapse uses the `role: "tool"` message's `name` as the Vertex function name,
so set `name` to the function's name as well as `tool_call_id`. See
[Streaming and tool calling](streaming-and-tools.md#tool-calling) for the full loop.

## Other request fields

On the native lane, Synapse also forwards `temperature` and `max_tokens` (as
`maxOutputTokens`). It sends `system` messages as user turns, not as a Vertex
`systemInstruction`. Other OpenAI fields, such as `top_p` or `stop`, are not forwarded.

## Timeouts

The native lane has no first-chunk or idle timeout. Each call is bounded only by the
provider HTTP timeout, `SYNAPSE_REQUEST_TIMEOUT_SECS` (default 120 seconds), which covers the
whole response, so a generation that runs longer fails even while it is still producing
output. Raise the value if you expect long responses. See
[Streaming and tool calling](streaming-and-tools.md#timeouts).

## Fallback between Vertex legs

When opening the stream fails with a `5xx`, `429` or `408` response, a connection error or a
timeout, Synapse tries the route's next `vertex` leg. Any other `4xx` stops the chain and
returns `400`. Once Vertex AI has accepted the request, a later failure is not retried, even
for non-streaming clients. See [Fallback chains](fallback-chains.md#native-vertex-lane).

## Gemini SDK clients

Clients that use Google's Gemini SDKs instead of an OpenAI SDK can call Synapse's
Gemini-native passthrough endpoints, `POST /v1beta/models/<model>:<action>`,
`POST /v1/models/<model>:<action>` and `POST /google/models/<model>:<action>`. Synapse
forwards the body verbatim to Vertex AI with its own credentials, meters `generateContent`
and `streamGenerateContent` calls in the ledger, and on a connection error or a `5xx`, `429`
or `408` response tries the following `vertex` legs of the route that lists that model.
`generateContent` and `streamGenerateContent` requests are scanned under the matched route's
policy, else `default`; see [What is scanned](../configuration/guardrails-policy.md#what-is-scanned).
`<model>` may be a route alias, which selects that route's legs and policy. A model that no
route lists gets `404` unless `SYNAPSE_PASSTHROUGH_MODELS=any`.
See [Gemini passthrough](../reference/http-api.md#gemini-passthrough) for the full
behaviour, including timeouts.
