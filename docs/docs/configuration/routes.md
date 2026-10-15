---
sidebar_position: 3
title: Routes
description: Reference for routes.toml, which maps the model names clients send to fallback chains of provider legs or to Jev-routed difficulty tiers.
---

`routes.toml` defines the model names your clients can send. Each route maps an alias to
the providers and models that serve it: either an ordered list of legs, or, with
`strategy = "jev"`, a set of difficulty tiers that Jev chooses between per request.

The gateway reads the file from `SYNAPSE_ROUTES_PATH` (default `config/routes.toml`) at
startup and refuses to start if it is missing or invalid. Restart the gateway to apply
changes. `GET /v1/models` lists every alias.

## Static routes

A static route is a table named after its alias with a list of legs:

```toml
[routes."chat"]
legs = [
  { provider = "vertex", model = "gemini-3.5-flash-lite", region = "us" },
  { provider = "openai", model = "gpt-4o-mini" },
]
```

Clients select it by sending `"model": "chat"`. Quote the alias in the table name when it
contains characters other than letters, digits, `-` and `_`.

| Key | Required | Description |
|---|---|---|
| `legs` | Yes, for static routes | The fallback chain: legs tried in order until one succeeds. |
| `policy` | No | Name of the guardrail policy to apply. Without it, the `default` policy applies. See [Guardrails policy](guardrails-policy.md). |
| `strategy` | No | `static` (the default) or `jev`. See [Jev routes](#jev-routes). |

### Leg keys

| Key | Required | Description |
|---|---|---|
| `provider` | Yes | `vertex`, `openai`, `qwen`, `oai_compat` or `typesafe`. See [Providers](providers.md). |
| `model` | Yes | The provider's model name, sent as is. It is also the model half of the `provider:model` key in `pricing.toml`. |
| `region` | No | Vertex location for this leg on the native Vertex lane, such as `global`, `us` or `us-central1`. Ignored on other lanes. |

## Fallback

Synapse tries a route's legs in order and returns the first successful response; the
client never sees the failed attempts. What counts as a failure depends on the lane:

- On the **standard lane**, any failure moves to the next leg: an error response, no first
  chunk within `SYNAPSE_REQUEST_TIMEOUT_SECS`, or a stream that stalls or breaks.
- On the **native Vertex lane**, only the route's `vertex` legs take part, and only a
  `5xx`, `429` or `408` response, a connection error or a timeout moves on. Any other `4xx`
  stops the chain.
- A **streaming** response can fall back only until its first chunk reaches the client.

When every leg of a standard-lane request fails, the client receives `502` with error code
`all_legs_failed` and a `failures` array naming each leg and its error. The
[fallback tutorial](../get-started/tutorials/fallback-across-providers.md) walks through
these cases, and the [fallback chains guide](../guides/fallback-chains.md) covers how to
design a chain.

## Regions

A leg's `region` applies only on the native Vertex lane. A native Vertex leg without a
`region` uses `VERTEX_LOCATION` (default `global`). Use it to pin a model to the location
that serves it, for example `global` for Gemini preview models, without changing the
process-wide default. Standard-lane Vertex calls always use the `global` endpoint.

A Vertex context cache must be in the same location as the leg that uses it.

## Jev routes

A route with `strategy = "jev"` declares difficulty tiers instead of legs. For each request,
Synapse asks TypeSafe Jev how demanding the conversation is, scored against the tier
descriptions, and whether it needs step-by-step reasoning. The nearest tier serves the
request with its reasoning `effort`, raised one step when reasoning is likely.

A `jev` route has a `strategy`, a `jev_router` table and one `[[routes."<alias>".tiers]]`
table per tier:

```toml
[routes."auto"]
strategy = "jev"

[routes."auto".jev_router]
default_tier = "moderate"

[[routes."auto".tiers]]
name = "moderate"
description = "Everyday Q&A, summarising, simple extraction or code edits"
effort = "low"
legs = [{ provider = "vertex", model = "gemini-3.6-flash", region = "global" }]

# ... at least one more tier
```

See the [complete example](#complete-example) for a working three-tier route.

A `jev` route needs `TYPESAFE_API_KEY` under strict provider validation. See
[Providers](providers.md#strict-and-lenient-validation) for what lenient validation does
without it.

### `jev_router` keys

| Key | Default | Description |
|---|---|---|
| `default_tier` | Required | Tier that serves when Jev times out, fails, or answers with low confidence. Must name a tier. |
| `model` | `jev-latest` | Jev model that makes the decision. Decision rows in the ledger are priced as `typesafe:<model>`. |
| `timeout_ms` | `400` | Time limit for the decision call, in milliseconds. Must be greater than 0. |
| `min_confidence` | `0.5` | Below this confidence, `default_tier` serves. Between 0 and 1. |
| `reasoning_threshold` | `0.7` | At or above this probability that the request needs reasoning, the effort is raised one step. Between 0 and 1. |

### Tier keys

Declare 2 to 10 `[[routes."<alias>".tiers]]` tables, ordered from easiest to hardest.

| Key | Description |
|---|---|
| `name` | Unique, non-empty, printable ASCII (spaces allowed). Returned in the `x-synapse-tier` response header. |
| `description` | The kind of work this tier suits. Jev scores requests against it, so describe the work, never the model. |
| `effort` | Reasoning effort for the tier's legs: `none`, `minimal`, `low`, `medium`, `high`, `xhigh` or `max`. |
| `legs` | The tier's legs, in the same form as a static route's. Any provider except `typesafe`. |

### Effort

On the standard lane, Synapse hands a tier's effort to the `genai` crate as its
`ReasoningEffort` option, and genai translates it for each provider. On the native Vertex
lane, Synapse sets `thinkingBudget` itself:

| Effort | `openai`, `qwen`, `oai_compat`: `reasoning_effort` | `vertex`, standard lane, Gemini 3: `thinkingLevel` | `vertex`, native lane: `thinkingBudget` |
|---|---|---|---|
| `none` | not sent | not sent | not sent |
| `minimal` | `minimal` | `MINIMAL` | 512 |
| `low` | `low` | `LOW` | 1024 |
| `medium` | `medium` | `MEDIUM` | 4096 |
| `high` | `high` | `HIGH` | 8192 |
| `xhigh` | `xhigh` | `HIGH` | 16384 |
| `max` | `xhigh` | `HIGH` | 24576 |

On the standard lane, Vertex models whose name does not contain `gemini-3` get a
`thinkingBudget` from genai instead: 1000 tokens for `minimal` and `low`, 8000 for `medium`,
and 24000 for `high`, `xhigh` and `max`.

With `none`, the model's default applies; on some Gemini models that default is dynamic
thinking, which can cost more than `minimal`. A client's own effort always wins on its
lane: `reasoning_effort` on the standard lane, translated the same way, and
`vertex.thinking_config` on the native Vertex lane.

The [Jev router guide](../guides/jev-router.md#effort) explains how to choose an effort for
each tier.

### Tier fallback

The chosen tier's legs run first, then each harder tier's, then each easier tier's. A
request with native Vertex features only uses `vertex` legs; if the chosen tier has none,
the nearest tier that does serves it, harder tiers first. Clients can send
`"routing_strategy": "static"` to skip the decision and start at `default_tier`.

A `jev` route returns `400` to requests carrying a `jev` block (`questions` or `extract`).

## Embedding aliases

Embedding aliases for `POST /v1/embeddings` live in the same file, under a separate
`embeddings` table with a declared output size:

```toml
[embeddings."embed"]
dimensions = 768
legs = [{ provider = "vertex", model = "text-embedding-004" }]
```

`dimensions` must be greater than 0 and `legs` must not be empty. Embedding legs support the
`vertex` and `openai` providers. See the [embeddings guide](../guides/embeddings.md) for
fallback, cost and attribution.

## Complete example

This file combines a single-leg route, a two-provider route with a guardrail policy, and a
three-tier Jev route with every `jev_router` key spelled out:

```toml title="config/routes.toml"
[routes."gemini-flash"]
legs = [
  { provider = "vertex", model = "gemini-3.5-flash-lite", region = "us" },
]

[routes."chat"]
policy = "strict"
legs = [
  { provider = "vertex", model = "gemini-3.5-flash-lite", region = "us" },
  { provider = "openai", model = "gpt-4o-mini" },
]

[routes."auto"]
strategy = "jev"

[routes."auto".jev_router]
default_tier = "moderate"
model = "jev-latest"
timeout_ms = 400
min_confidence = 0.5
reasoning_threshold = 0.7

[[routes."auto".tiers]]
name = "trivial"
description = "Greetings, chit-chat, one-line lookups or rewrites"
effort = "none"
legs = [{ provider = "vertex", model = "gemini-3.5-flash-lite", region = "us" }]

[[routes."auto".tiers]]
name = "moderate"
description = "Everyday Q&A, summarising, simple extraction or code edits"
effort = "low"
legs = [{ provider = "vertex", model = "gemini-3.6-flash", region = "global" }]

[[routes."auto".tiers]]
name = "hard"
description = "Multi-step analysis, maths or proofs, non-trivial code or debugging"
effort = "medium"
legs = [{ provider = "vertex", model = "gemini-3.1-pro-preview", region = "global" }]
```

## Validation

The gateway checks the file at startup and stops with a message naming the route when:

- a static route has no `legs` key, or declares `tiers` without `strategy = "jev"`;
- `strategy` is neither `static` nor `jev`;
- a `jev` route has non-empty `legs`, no `jev_router` table, fewer than 2 or more than 10 tiers, or a
  `default_tier` that is not one of its tiers;
- a tier has an empty or non-ASCII name, a duplicate name, an empty description, no legs, a
  `typesafe` leg or an unknown `effort`;
- `min_confidence` or `reasoning_threshold` is outside 0 to 1, or `timeout_ms` is 0.

:::warning
Unknown keys are ignored, not rejected. A misspelled key, such as `polcy`, silently has no
effect.
:::

Provider credentials are checked after the file loads; see
[Providers](providers.md#strict-and-lenient-validation).

## Embedding policy

An embedding alias can name a guardrails policy. Without it, embeddings use `default`:

```toml title="config/routes.toml"
[routes."chat"]
legs = [{ provider = "vertex", model = "gemini-3.5-flash-lite", region = "us" }]

[embeddings."docs-embed"]
dimensions = 768
policy = "embeddings"
legs = [{ provider = "vertex", model = "text-embedding-004" }]
```

The named policy must be defined in `guardrails.toml`; under the default
`SYNAPSE_GUARDRAILS=required` the gateway refuses to start otherwise.

## Gemini passthrough by alias

A Gemini SDK client can name a route alias as its model. The gateway walks that route's
leading `vertex` legs, sends each leg's own model to Vertex, and scans under the route's
policy. `strategy = "jev"` routes are not available on the passthrough. An alias whose route
has no leading `vertex` legs gets `400`.
