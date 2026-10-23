---
sidebar_position: 3
title: Concepts
description: The terms used throughout the Synapse documentation, from routes and legs to lanes, tiers and the cost ledger.
---

This page defines the terms the rest of the documentation uses. Each entry is short; follow
the links for detail.

## Route

A named entry in `routes.toml`, such as `[routes."chat"]`. Clients select a route by sending
its name, the **alias**, as the `model` field of a chat completion request, and
`GET /v1/models` lists every alias. An unknown alias returns `404` with error code
`model_not_found`. See [Request flow](architecture.md#request-flow) and
[Routes](../configuration/routes.md).

## Leg

One provider and model in a route, for example
`{ provider = "vertex", model = "gemini-3.5-flash-lite", region = "us" }`. The provider
is one of `vertex`, `openai`, `qwen`, `oai_compat` or `typesafe`. The optional `region` pins
the leg to a Vertex location on the native Vertex lane. See
[Leg keys](../configuration/routes.md#leg-keys), [Regions](../configuration/routes.md#regions)
and [Providers](../configuration/providers.md).

## Lane

The backend path that serves a request: **standard** (OpenAI-compatible providers through
the `genai` crate), **native Vertex** (the Vertex REST API) or **Jev** (TypeSafe System One).
The request body selects the lane. See [Lanes](architecture.md#lanes) and
[Lane detection](architecture.md#lane-detection), and the
[native Vertex](../guides/native-vertex.md) and [Jev lane](../guides/jev-lane.md) guides.

## Strategy

How a route chooses its legs, set with the route's `strategy` key. `static`, the default,
uses the route's `legs` in order. `jev` declares tiers instead of legs, and the Jev router
picks a tier for each request. A client can send `"routing_strategy": "static"` to skip the
decision for one request on a `jev` route. See [Routes](../configuration/routes.md) and
[Override the decision per request](../guides/jev-router.md#override-the-decision-per-request).

## Tier

One difficulty level of a `strategy = "jev"` route, declared as `[[routes."<alias>".tiers]]`
with a `name`, a `description` of the work it suits, a reasoning `effort` and its own `legs`.
A route has 2 to 10 tiers, ordered from easiest to hardest. The `default_tier` key in the
route's `[routes."<alias>".jev_router]` table names the tier that serves when Jev cannot
decide. See the [Jev router guide](../guides/jev-router.md) and
[Tier keys](../configuration/routes.md#tier-keys).

## Fallback chain

The ordered legs Synapse tries for a request until one succeeds. On a static route it is the
route's `legs`; on a `jev` route it starts at the chosen tier and continues through harder
tiers, then easier ones. See [Request flow](architecture.md#request-flow) and the
[fallback chains guide](../guides/fallback-chains.md).

## Tenant

The customer, team or application a request is attributed to, taken from the
`x-synapse-tenant` header. Without the header, Synapse uses `SYNAPSE_DEFAULT_TENANT`
(default `unattributed`). The tenant is recorded on ledger rows, but it is never a metric
label, because clients control its value. See
[Tenant attribution](../guides/tenant-attribution.md) and
[Tenant headers are trusted](../operating/security.md#tenant-headers-are-trusted).

## AI task type

A label on every ledger row describing the kind of work the gateway performed. Synapse uses
the `x-synapse-ai-task-type` header when present, otherwise the task type mapped to the
route alias in `config/ai_task_types.toml`, otherwise `simple`. See
[AI task types](../guides/tenant-attribution.md#ai-task-types) and
[`ai_task_types.toml`](../reference/configuration-keys.md#ai_task_typestoml).

## Cost ledger

The record of token usage and cost for each completed request, written asynchronously so it
never slows the response. Cost comes from `pricing.toml` (USD per 1,000,000 tokens, keyed by
`provider:model`); unlisted models cost 0. Rows go to SQLite by default, or to Postgres, and
can also be published to Google Cloud Pub/Sub and AWS SNS. See the
[cost ledger guide](../guides/cost-ledger.md) and [Pricing](../configuration/pricing.md).

## Guardrail policy

A named security policy defined under `[guardrails.<name>]` in `guardrails.toml` that can
combine in-process input span scanners (`prompt_injection`, `secrets`, `pii`,
`isolation_bypass`, `data_exfil` and more), in-place `[REDACTED:<scanner>]` masking
(`action = "mask"`), an optional TypeSafe Jev semantic `classifier`, and `output_scanners`
for buffered and streaming replies. A policy either enforces blocks and masks (`mode = "block"`,
the default) or runs in dry-run shadow mode (`mode = "observe"`), logging and counting what it
would have caught without blocking or modifying traffic. A route opts in with
`policy = "<name>"`; routes without one use the `default` policy, and without a `default`
policy those routes are not scanned. Without `guardrails.toml` the gateway refuses to start,
unless `SYNAPSE_GUARDRAILS=optional`. See
[Guardrails policy](../configuration/guardrails-policy.md) and
[Where and how Jev is used](../configuration/guardrails-policy.md#where-and-how-jev-is-used).
