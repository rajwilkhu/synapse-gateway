---
sidebar_position: 6
title: Embeddings
description: Serve OpenAI-compatible POST /v1/embeddings through dimension-pinned fallback legs on Vertex AI and OpenAI, with per-tenant cost accounting.
---

Use Synapse's embeddings endpoint when you build or query a vector index and want the same
routing, fallback and per-tenant cost accounting you get for chat. Clients call the
OpenAI-compatible `POST /v1/embeddings` with an alias; Synapse serves it from a Vertex AI or
OpenAI-compatible embedding model and guarantees that every leg returns vectors of the same
length, so a failover never writes mismatched vectors into your index.

## Define an alias

Embedding aliases live in `routes.toml`, next to your chat routes, under a separate
`embeddings` table. Each alias declares its vector size in `dimensions` and an ordered list
of legs:

```toml
[embeddings."embed"]
dimensions = 768
legs = [
  { provider = "vertex", model = "text-embedding-004" },
  { provider = "openai", model = "text-embedding-3-small" },
]
```

Synapse pins `dimensions` on every leg: it sends Vertex AI `outputDimensionality` and OpenAI
`dimensions`, so both legs above return 768 floats, even though `text-embedding-3-small`
returns 1,536 by default. Every model in an alias must support reducing its output to that
size; older fixed-size models such as `textembedding-gecko` or `text-embedding-ada-002`
cannot join a pinned alias. [Embedding aliases](../configuration/routes.md#embedding-aliases)
lists the validation rules.

:::warning
Vectors from different models are not comparable, even at the same length. A fallback leg
keeps your index from breaking, but its vectors sit in a different space from your primary
model's. If similarity quality matters more than availability, use a single-leg alias, or
record which model produced each vector and re-embed after a failover.
:::

## Providers

Embedding legs support two providers:

- **`vertex`** calls the Vertex AI `:predict` endpoint in the project from
  `VERTEX_PROJECT_ID` and the location from `VERTEX_LOCATION` (default `global`), with the
  same credentials as chat. A leg's `region` is not used for embeddings.
- **`openai`** calls `POST <OPENAI_BASE_URL>/embeddings` with `OPENAI_API_KEY`. Point
  `OPENAI_BASE_URL` at any OpenAI-compatible embeddings server to use it instead.

A missing credential for a referenced provider stops the gateway at startup under strict
validation; lenient validation drops those legs. See [Providers](../configuration/providers.md#embedding-aliases).

## Send a request

```bash
curl -s http://localhost:8080/v1/embeddings \
  -H "Content-Type: application/json" \
  -H "x-synapse-tenant: my-team" \
  -d '{ "model": "embed", "input": ["hello world", "second chunk"] }'
```

The response has one vector of 768 floats per input, shortened here:

```json
{
  "object": "list",
  "data": [
    { "object": "embedding", "index": 0, "embedding": [0.0123, -0.0456, 0.0311] },
    { "object": "embedding", "index": 1, "embedding": [0.0789, 0.0012, -0.0204] }
  ],
  "model": "embed",
  "usage": { "prompt_tokens": 7, "total_tokens": 7 }
}
```

- `input` is a string or an array of strings. Synapse splits large arrays into batches of
  250 for Vertex AI and 2,048 for OpenAI, and returns the vectors in input order.
- `dimensions` is optional. If you send it, it must equal the alias's `dimensions`, or the
  request fails with `400`: the alias, not the client, decides the vector size.
- `model` in the response is the alias, not the model that served it.
- An unknown alias returns `404` with error code `model_not_found`. Embedding aliases are not
  listed by `GET /v1/models`, which lists chat routes only.

## Fallback

Synapse tries the legs in order. A leg whose provider this process has not configured is
skipped, and any error, including a `4xx`, moves to the next leg. If a large input needs
several batches and one fails, the whole leg fails and the next leg embeds every input
again, so a response never mixes vectors from two models. When every leg fails, the last
leg's error is returned, usually `502` with error code `upstream_error`.

Embedding calls have no retries of their own; each leg gets one attempt, bounded by
`SYNAPSE_REQUEST_TIMEOUT_SECS`.

## Cost

Embedding usage counts input tokens only. Synapse prices it with the `input` price of the
leg's `provider:model` entry in `pricing.toml`:

```toml
"vertex:text-embedding-004" = { input = 0.025, output = 0.0 }
"openai:text-embedding-3-small" = { input = 0.02, output = 0.0 }
```

Unlike chat, an embedding model without an entry is not free: it is priced at
`SYNAPSE_EMBED_DEFAULT_INPUT_PRICE_PER_MTOK` (default `0.10` USD per 1,000,000 tokens), so
usage is never silently recorded as zero cost. See [Pricing](../configuration/pricing.md#how-cost-is-computed).

Token counts come from the provider: the per-input `statistics.token_count` from Vertex AI,
and `usage.total_tokens` from OpenAI.

## Attribution and the ledger

Embedding requests take the same attribution headers as chat, such as `x-synapse-tenant`
and `x-synapse-workspace`; see [Tenant attribution](tenant-attribution.md). Each successful
request writes one ledger row with lane `embedding`, the serving leg's provider and model,
and `output_tokens` of 0. In events published to Pub/Sub or SNS, the row has
`op = "embedding"`. See [Cost ledger](cost-ledger.md).

Each `input` item is scanned under the alias's `policy`, else `default`, before the first
leg; see [What is scanned](../configuration/guardrails-policy.md#what-is-scanned).

## Metrics

| Metric | Labels | Description |
|---|---|---|
| `synapse_embeddings_total` | `route`, `model`, `provider` | Embedding requests served. `route` is the alias. |
| `synapse_embedding_duration_seconds` | `route`, `model`, `provider` | Latency of the leg that served the request. |

## In-process embeddings

Applications that embed the gateway call `Gateway::embed` directly; see
[Embedding Synapse as a library](embedding-as-library.md#embeddings).
