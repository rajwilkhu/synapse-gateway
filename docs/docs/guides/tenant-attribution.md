---
sidebar_position: 7
title: Tenant attribution
description: Attribute every request to a tenant, workspace, user, thread and task type with x-synapse-* headers, and classify the work with AI task types.
---

Use tenant attribution when more than one team, customer or application shares a gateway
and you need to know who spent what. Clients add `x-synapse-*` headers to their requests,
and Synapse records them on every [cost ledger](cost-ledger.md) row, so you can report usage
and cost per tenant, per workspace, per end user or per kind of work without changing the
request body.

## Attribution headers

| Header | Recorded as | Description |
|---|---|---|
| `x-synapse-tenant` | `tenant` | The customer, team or application. Without the header, `SYNAPSE_DEFAULT_TENANT` (default `unattributed`). |
| `x-synapse-workspace` | `workspace` | A grouping within the tenant, such as a project or team. |
| `x-synapse-user` | `user_id` | The end user within the tenant. |
| `x-synapse-thread` | `thread_id` | A conversation or agent thread. |
| `x-synapse-message` | `message_id` | A message or unit of work within the thread. Also becomes the request id; see [Correlate requests](#correlate-requests). |
| `x-synapse-user-task-type` | `user_task_type` | Your own label for the work the request serves, such as `summarisation`. Recorded as sent and never interpreted. |
| `x-synapse-ai-task-type` | `ai_task_type` | Overrides the [AI task type](#ai-task-types) for this request. |

All headers are optional. They work the same on `POST /v1/chat/completions`,
`POST /v1/embeddings` and the passthrough endpoints. For example:

```bash
curl -s http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "x-synapse-tenant: my-team" \
  -H "x-synapse-workspace: onboarding" \
  -H "x-synapse-user: user-42" \
  -H "x-synapse-user-task-type: summarisation" \
  -d '{
    "model": "chat",
    "messages": [{"role": "user", "content": "Summarise our onboarding guide."}]
  }'
```

Synapse records the values as sent. A header that is present but empty records an empty
string rather than falling back to the default.

:::warning
Synapse does not authenticate these headers: any client can claim any tenant. If
attribution drives billing or quotas, run Synapse behind a gateway or service mesh that
authenticates callers and sets `x-synapse-tenant` itself, overwriting any value the client
sent.
:::

## Where attribution goes

Every ledger row has a column for each of the seven fields. `tenant` and `ai_task_type` are
always set; the others are null when the header was not sent. Events published to Pub/Sub or
SNS carry the same fields in camelCase, omitting absent ones, with the tenant as
`namespace`. See [Cost ledger](cost-ledger.md) for the full
row and event format.

None of them are metric labels. Their values come from clients and are unbounded, and
putting them on metrics would create a new series for every tenant, user or thread. Query
the ledger for per-tenant numbers, and use the metrics for per-route, per-model and per-lane
views.

Set `SYNAPSE_DEFAULT_TENANT` to something recognisable, such as the name of the deployment,
so unattributed usage stands out. See [Environment variables](../configuration/environment-variables.md#tenancy).

## Correlate requests

Each request gets a request id, recorded as the ledger's `request_id`. For chat
completions it is also the response `id`, as `chatcmpl-<request id>`. When the request has an `x-synapse-message`
header, that value is the request id; otherwise Synapse generates a UUID.

Stamping `x-synapse-message` lets you join a ledger row to the message in your own system.
All rows a single request writes share its request id: on a `strategy = "jev"` route, the
routing decision and the chat completion; with hybrid extraction, the Jev answer and every
extraction. Reusing one message id for several requests gives their rows the same
`request_id`.

## AI task types

The AI task type classifies the work the gateway performed, such as `conversation`,
`extraction` or `vision`, so you can report cost by kind of work across routes and tenants.
Every ledger row has one. Synapse resolves it per request:

1. the `x-synapse-ai-task-type` header, when present and non-empty;
2. otherwise the task type mapped to the request's route alias in `ai_task_types.toml`;
3. otherwise `simple`.

`ai_task_types.toml` is keyed by task type, each listing the route aliases that perform
that kind of work:

```toml
conversation = ["chat", "support-bot"]
extraction = ["invoice-extract"]
vision = ["receipt-ocr"]
```

- The gateway reads it from `SYNAPSE_AI_TASK_TYPES_PATH` (default
  `config/ai_task_types.toml`) at startup. The file is optional: without it, every request
  resolves to `simple` unless the header says otherwise.
- Aliases not listed resolve to `simple`, so list only the routes whose work is not simple.
- An alias listed under two task types stops the gateway at startup; listing it twice under
  the same type is allowed.
- For embeddings, the alias is the embedding alias.

The two task type fields answer different questions. `user_task_type` is your application's
own label, recorded only when a client sends it. `ai_task_type` is always set, and is
controlled centrally in `ai_task_types.toml`, so reports stay consistent even when clients
don't send anything.

## Multi-tenancy with organisations and workspaces

In multi-tenant deployments where multiple customer organisations (`tenant` / `org`), teams or
projects (`workspace`), and end users share Synapse, the workspace crates work together to
**bind**, **enforce**, **guard**, and **attribute** every call across the two-level
`(organisation, workspace)` hierarchy.

### Tenancy hierarchy and cross-crate mapping

```text
Organisation (`tenant` / `org` / `namespace`)          e.g. "acme"
  └── Workspace (`workspace`)                          e.g. "onboarding", "prod-support"
        └── User (`user_id` / `user`)                  e.g. "user-42", "agent-triage"
              └── Thread (`thread_id` / `threadId`)    e.g. "thread-9001"
                    └── Message (`message_id` / `request_id`)  e.g. "msg-001"
```

| Level | `synapse-gateway` header | `RequestCtx` field | `usage_events` column (SQLite / Postgres) | Pub/Sub & SNS JSON field | `synapse-context` / `synapse-proxy` / `synapse-mcp` key |
|---|---|---|---|---|---|
| **Organisation** | `x-synapse-tenant` | `tenant` | `tenant` (defaults to `SYNAPSE_DEFAULT_TENANT`) | `namespace` (also a message filter attribute) | `org` |
| **Workspace** | `x-synapse-workspace` | `workspace` | `workspace` | `workspace` | `workspace` |
| **User** | `x-synapse-user` | `user` | `user_id` | `user` | `user` |
| **Thread** | `x-synapse-thread` | `thread` | `thread_id` | `threadId` | — |
| **Message** | `x-synapse-message` | `message` | `message_id` and `request_id` | `messageId` and `requestId` | — |

### Enforcing org and workspace identity

Because `synapse-gateway` trusts incoming `x-synapse-*` headers, untrusted callers (such as
external clients, code sandboxes, or agent containers) must never reach the gateway directly.
Use one of two enforcement patterns:

1. **Edge API gateway / ingress (multi-tenant HTTP API)**: Your authenticating ingress or API
   gateway verifies the caller's token, strips any client-supplied `x-synapse-*` headers, and
   stamps `x-synapse-tenant: <org>` and `x-synapse-workspace: <workspace>` before forwarding to
   `synapse-gateway`.
2. **Untrusted sandbox / agent sidecar (`synapse-context` + `synapse-proxy` + `synapse-mcp`)**:
   Your control plane binds the active `(org, workspace, user)` context once into a shared
   `ContextStore` via `POST /internal/bind`. Both `synapse-proxy` (for LLM and HTTP calls) and
   `synapse-mcp` (for MCP tool calls) read that store, overwrite any spoofed headers from the
   sandbox, and **fail closed** when the context is unbound.

```text
Control Plane
  │  POST 127.0.0.1:8788/internal/bind
  │  {"values": {"org": "acme", "workspace": "onboarding", "user": "user-42"}, "ttl_seconds": 3600}
  ▼
┌────────────────────────────────────────────────────────────────────────────┐
│ Shared `ContextStore` (`synapse-context`)                                  │
│   base (static ⊕ env) + live TTL overlay → ResolvedContext                 │
└───────────────────┬────────────────────────────────────┬───────────────────┘
                    │                                    │
                    ▼                                    ▼
┌──────────────────────────────────────┐  ┌──────────────────────────────────┐
│ `synapse-proxy` (:8787)              │  │ `synapse-mcp` (/mcp/{server})    │
│ • `require_context = ["org",         │  │ • `required = true` on `org` and │
│    "workspace"]` (503 if unbound)    │  │   `workspace` (fails closed)     │
│ • Overwrites caller headers with:    │  │ • Opens dedicated connection per │
│   `x-synapse-tenant: acme`           │  │   `(server, identity)` with:     │
│   `x-synapse-workspace: onboarding`  │  │   `x-org-id: acme`               │
│   `x-synapse-user: user-42`          │  │   `x-workspace-id: onboarding`   │
└───────────────────┬──────────────────┘  └─────────────────┬────────────────┘
                    │                                       │
                    ▼                                       ▼
┌──────────────────────────────────────┐          Upstream MCP Servers
│ `synapse-gateway` (:8080)            │
│ • Guardrails (`isolation_bypass` &   │
│   `data_exfil`) block cross-org /    │
│   cross-workspace escape attempts    │
│ • Cost ledger records `tenant="acme"`│
│   and `workspace="onboarding"`       │
└──────────────────────────────────────┘
```

Example `synapse-proxy.toml` stamping `org` and `workspace` onto requests forwarded to
`synapse-gateway`:

```toml title="synapse-proxy.toml"
addr = "127.0.0.1:8787"
admin_addr = "127.0.0.1:8788"
metrics_addr = "127.0.0.1:9091"

[context]
env = { org = "TENANT_ORG_ID", workspace = "TENANT_WORKSPACE_ID" }

[[routes]]
name = "llm-gateway"
path_prefix = "/v1"
upstream = "http://127.0.0.1:8080"
strip_prefix = false
require_context = ["org", "workspace"]
request_steps = [
  { inject = { header = "x-synapse-tenant",    from_context = "org" } },
  { inject = { header = "x-synapse-workspace", from_context = "workspace" } },
  { inject = { header = "x-synapse-user",      from_context = "user" } },
]
```

See [Context and transforms](../synapse-family/proxy/context-and-transforms.md) and
[Identity injection](../synapse-family/mcp/identity-injection.md).

### Guarding against cross-tenant and cross-workspace escapes

Pair header enforcement with the multi-tenant scanners in `config/guardrails.toml` so a
prompt-injected model or tool output cannot emit instructions or commands that cross org or
workspace boundaries:

- **[`isolation_bypass`](../configuration/guardrails-policy.md#isolation_bypass)**: Detects
  attempts in prompts or model replies to set identity headers (`x-tenant-id`,
  `x-workspace-id`, `x-user-id`, `x-agent-id`) via `curl -H`, header dictionaries, or setter
  calls, or to reach internal Kubernetes services (`*.svc.cluster.local`).
- **[`data_exfil`](../configuration/guardrails-policy.md#data_exfil)**: Detects unbounded
  workspace traversal such as root-scoped `cortex://<tenant>/<workspace>/` URIs or
  `recursive=true` on `/fs/ls`.

```toml title="config/guardrails.toml"
[guardrails.multi-tenant]
mode = "block"
scanners = [
  "prompt_injection",
  { type = "secrets", action = "mask" },
  { type = "pii", action = "mask" },
  "isolation_bypass",
  "data_exfil",
]
output_scanners = [
  { type = "secrets", action = "mask" },
  "isolation_bypass",
  "data_exfil",
]
```

### Querying spend by organisation and workspace

Break down daily spend and token usage by organisation (`tenant`) and `workspace`:

```sql
SELECT tenant AS org,
       COALESCE(workspace, '(none)') AS workspace,
       substr(ts, 1, 10) AS day,
       COUNT(*) AS requests,
       SUM(input_tokens) AS input_tokens,
       SUM(output_tokens) AS output_tokens,
       SUM(cost_usd) AS usd
FROM usage_events
WHERE status = 'ok'
GROUP BY tenant, workspace, day
ORDER BY day DESC, usd DESC;
```

Count blocked guardrail requests per organisation and workspace:

```sql
SELECT tenant AS org,
       COALESCE(workspace, '(none)') AS workspace,
       model AS policy_or_classifier,
       COUNT(*) AS blocked_requests
FROM usage_events
WHERE lane = 'guard' AND status = 'blocked'
GROUP BY tenant, workspace, model
ORDER BY blocked_requests DESC;
```

## In an embedded gateway

Applications that embed the gateway pass the same fields in a `RequestCtx`, whose `tenant`,
`workspace`, `user`, `thread`, `message`, `user_task_type` and `ai_task_type` fields match the
headers, plus an optional `request_id`. See [Embedding Synapse as a library](embedding-as-library.md).
