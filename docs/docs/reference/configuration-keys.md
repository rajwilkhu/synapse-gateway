---
sidebar_position: 2
title: Configuration keys
description: Every key of every gateway configuration file, with its type, whether it is required and its default.
---

This page lists every key the gateway reads from its TOML files, on one page for lookup. The
[configuration pages](../configuration/environment-variables.md) explain what the keys do and
how they combine; each section below links to its page.

The gateway reads every file once at startup; restart it to apply a change. Keys the gateway
doesn't know are ignored without an error, so a misspelled optional key silently has no effect.

## Files

| File | Path variable | Default path | If the file is missing |
|---|---|---|---|
| `routes.toml` | `SYNAPSE_ROUTES_PATH` | `config/routes.toml` | The gateway doesn't start. |
| `pricing.toml` | `SYNAPSE_PRICING_PATH` | `config/pricing.toml` | The gateway doesn't start. |
| `guardrails.toml` | `SYNAPSE_GUARDRAILS_PATH` | `config/guardrails.toml` | The gateway doesn't start, unless `SYNAPSE_GUARDRAILS=optional`, which runs with guardrails off. |
| `ai_task_types.toml` | `SYNAPSE_AI_TASK_TYPES_PATH` | `config/ai_task_types.toml` | Every request records the AI task type `simple`. |
| `a2a.toml` | `SYNAPSE_A2A_PATH` | `config/a2a.toml` | The A2A registry starts empty. |

A file that exists but doesn't parse, or fails validation, stops the gateway at startup with a
message naming the problem. Environment variables are listed in
[Environment variables](../configuration/environment-variables.md).

## `routes.toml`

Chat routes and embedding aliases. See [Routes](../configuration/routes.md).

The file must contain a `routes` table, even if every alias you need is an embedding alias. A
file with only `[embeddings.*]` tables fails to load.

### `[routes."<alias>"]`

| Key | Type | Required | Default | Description |
|---|---|---|---|---|
| `legs` | array of leg tables | Yes, on static routes | — | The fallback chain. Must be absent or empty on a `jev` route. |
| `strategy` | string | No | `"static"` | `"static"` or `"jev"`. |
| `policy` | string | No | the `default` policy | Guardrail policy name from `guardrails.toml`. |
| `jev_router` | table | Yes, on `jev` routes | — | Decision settings. See [below](#routesaliasjev_router). |
| `tiers` | array of tier tables | Yes, on `jev` routes | — | 2 to 10 tiers, easiest first. See [below](#routesaliastiers). |

### Leg table

Used in `legs`, in each tier's `legs`, and in embedding aliases.

| Key | Type | Required | Default | Description |
|---|---|---|---|---|
| `provider` | string | Yes | — | `vertex`, `openai`, `qwen`, `oai_compat` or `typesafe`. Embedding legs support `vertex` and `openai`. See [Providers](../configuration/providers.md). |
| `model` | string | Yes | — | The provider's model name, sent as is. |
| `region` | string | No | `VERTEX_LOCATION` | Vertex location for this leg on the native Vertex lane. Ignored elsewhere. |

### `[routes."<alias>".jev_router]`

| Key | Type | Required | Default | Description |
|---|---|---|---|---|
| `default_tier` | string | Yes | — | Tier used when Jev can't decide. Must name a tier. |
| `model` | string | No | `"jev-latest"` | Jev model that makes the decision. |
| `timeout_ms` | integer | No | `400` | Decision time limit in milliseconds. Greater than 0. |
| `min_confidence` | float | No | `0.5` | Below this confidence, `default_tier` serves. 0 to 1. |
| `reasoning_threshold` | float | No | `0.7` | At or above this, the effort is raised one step. 0 to 1. |

See [`jev_router` keys](../configuration/routes.md#jev_router-keys).

### `[[routes."<alias>".tiers]]`

| Key | Type | Required | Default | Description |
|---|---|---|---|---|
| `name` | string | Yes | — | Unique, non-empty, printable ASCII. |
| `description` | string | Yes | — | The work this tier suits. Non-empty. |
| `effort` | string | Yes | — | `none`, `minimal`, `low`, `medium`, `high`, `xhigh` or `max`. |
| `legs` | array of leg tables | Yes | — | At least one leg; no `typesafe` legs. |

See [Tier keys](../configuration/routes.md#tier-keys).

### `[embeddings."<alias>"]`

| Key | Type | Required | Default | Description |
|---|---|---|---|---|
| `dimensions` | integer | Yes | — | Output vector size. Greater than 0. |
| `legs` | array of leg tables | Yes | — | At least one leg. |
| `policy` | string | No | the `default` policy | Guardrail policy name from `guardrails.toml`. |

See [Embedding aliases](../configuration/routes.md#embedding-aliases).

## `pricing.toml`

Prices for the cost ledger. See [Pricing](../configuration/pricing.md).

Every top-level key is a `"<provider>:<model>"` string, and its value is a table:

| Key | Type | Required | Description |
|---|---|---|---|
| `input` | float | Yes | USD per 1,000,000 input tokens. |
| `output` | float | Yes | USD per 1,000,000 output tokens. |

```toml
"openai:gpt-4o-mini" = { input = 0.15, output = 0.60 }
```

A top-level key whose value isn't such a table stops the gateway. An empty file is valid.

## `guardrails.toml`

Named guardrail policies. See [Guardrails policy](../configuration/guardrails-policy.md).

### `[guardrails.<name>]`

| Key | Type | Required | Default | Description |
|---|---|---|---|---|
| `scanners` | array | Yes | — | Scanner names, or scanner tables. |
| `mode` | string | No | `"block"` | `"block"` or `"observe"`. |

### Scanner table

A scanner is a bare name, such as `"secrets"`, or a table:

| Key | Type | Used by | Description |
|---|---|---|---|
| `type` | string | every scanner | Required. The scanner name: `prompt_injection`, `secrets`, `pii`, `invisible_text`, `role_override`, `token_limit`, `ban_substrings` or `script_mix`. |
| `max_chars` | integer | `token_limit` | Required for `token_limit`. Maximum input length in characters. |
| `substrings` | array of strings | `ban_substrings` | Required for `ban_substrings`, non-empty. |
| `severity` | string | `ban_substrings` | `block` (default), `warn` or `info`. |
| `threshold` | integer | `script_mix` | Characters outside the dominant script before flagging. Default `2`. |

See [Scanners](../configuration/guardrails-policy.md#scanners).

## `ai_task_types.toml`

Maps route aliases to the AI task type on ledger rows. See
[AI task types](../guides/tenant-attribution.md#ai-task-types).

Every top-level key is a task type name, and its value is an array of route aliases, chat or
embedding:

```toml
conversation = ["chat", "support-bot"]
extraction = ["invoice-extract"]
```

An alias listed under two task types stops the gateway. Aliases not listed resolve to `simple`.

## `a2a.toml`

Agents to register in the gateway's A2A registry at startup. Each agent is an `[[a2a_agents]]`
table:

| Key | Type | Required | Default | Description |
|---|---|---|---|---|
| `id` | string | Yes | — | Registry id, used in the `/a2a/agents/{id}/...` paths. |
| `name` | string | Yes | — | Display name in the catalogue. |
| `description` | string | Yes | — | Description in the catalogue. |
| `endpoint_url` | string | Yes | — | Absolute URL where the agent serves A2A. |
| `card_url` | string | Yes | — | Absolute URL of the agent's card. The gateway fetches it at startup. |
| `tags` | array of strings | No | `[]` | Catalogue tags. |
| `ttl_seconds` | integer | No | no expiry | Seconds after startup when the agent leaves the catalogue. |

```toml
[[a2a_agents]]
id = "invoice-agent"
name = "Invoice agent"
description = "Extracts line items from invoices"
endpoint_url = "https://agents.example.com/invoice"
card_url = "https://agents.example.com/invoice/.well-known/agent-card.json"
tags = ["finance"]
```

At startup the gateway fetches each agent's card from `card_url`, with a 15-second timeout.
Connection errors, `5xx` and `429` responses are retried twice with backoff. An agent whose
card can't be fetched is logged and skipped, and the gateway starts without it; a duplicate
`id` is also skipped. The registry and its endpoints are described in
[A2A agent registry](./http-api.md#a2a-agent-registry).
