---
sidebar_position: 2
title: Quickstart
description: Run the Synapse gateway with a two-route config and send standard, streaming, fallback and native Vertex AI requests to it.
---

In this quickstart you write a two-route configuration, start the gateway, and send it a
standard request, a streaming request, a request that can fall back to OpenAI and a native
Vertex AI request. Then you look up what each request cost.

## Prerequisites

- Docker, or Synapse installed with Cargo (see [Installation](installation.md)).
- A Google Cloud project with the Vertex AI API enabled, and credentials that can call it:
  a service-account key with the Vertex AI User role for Docker, or
  `gcloud auth application-default login` for a local binary.
- Optionally, an OpenAI API key for the second leg of the `chat` route. Without one, see the
  tip under [Start the gateway](#start-the-gateway).
- The `sqlite3` command-line tool on your machine, for the last step,
  [See what it cost](#see-what-it-cost).

The gateway starts without valid credentials: at startup it only checks that
`VERTEX_PROJECT_ID` and `OPENAI_API_KEY` are set. Credentials are used on the first request.
If they are missing or invalid, a standard request fails with `502` and error code
`all_legs_failed`. A native Vertex request fails with `502` and `upstream_error` when
credentials are missing, or with `400` when Vertex AI rejects them.

## Create the configuration

Create a working directory with a `config/` folder for the configuration and a `data/`
folder for the cost ledger:

```bash
mkdir -p synapse-quickstart/config synapse-quickstart/data && cd synapse-quickstart
```

Save the routes as `config/routes.toml`:

```toml title="config/routes.toml"
[routes."gemini-flash"]
legs = [
  { provider = "vertex", model = "gemini-3.5-flash-lite", region = "us" },
]

[routes."chat"]
legs = [
  { provider = "vertex", model = "gemini-3.5-flash-lite", region = "us" },
  { provider = "openai", model = "gpt-4o-mini" },
]
```

Each `[routes."<alias>"]` table defines a model name your clients can send. `gemini-flash`
always uses Gemini 3.5 Flash-Lite on Vertex AI. `chat` tries Gemini first and falls back to
OpenAI's `gpt-4o-mini` if the Vertex leg fails. `region` pins the Vertex leg to Vertex's
`us` multi-region on the native Vertex lane.

Save the prices as `config/pricing.toml`:

```toml title="config/pricing.toml"
"vertex:gemini-3.5-flash-lite" = { input = 0.30, output = 2.50 }
"openai:gpt-4o-mini" = { input = 0.15, output = 0.60 }
```

Prices are in USD per 1,000,000 tokens, keyed by `provider:model`. Synapse uses them to
price every request in the cost ledger; a model without a price costs 0.

Save the guardrails as `config/guardrails.toml`:

```toml title="config/guardrails.toml"
[guardrails.default]
scanners = ["prompt_injection", "secrets"]
```

A route without a `policy` is scanned under `default`, so both routes block prompt
injection and leaked secrets. The gateway refuses to start without this file; see
[Guardrails policy](../configuration/guardrails-policy.md).

## Start the gateway

The gateway runs in the foreground. Leave it running and send the requests in the rest of
this quickstart from a second terminal, in the same `synapse-quickstart` directory.

### Docker

Put your service-account key in the working directory as `sa.json`, then run:

```bash
docker run --rm -p 8080:8080 -p 9090:9090 \
  -e VERTEX_PROJECT_ID=my-gcp-project \
  -e OPENAI_API_KEY=sk-... \
  -e GOOGLE_APPLICATION_CREDENTIALS=/secrets/sa.json \
  -e SYNAPSE_LEDGER_SQLITE_DSN="sqlite:///app/data/synapse.db?mode=rwc" \
  -v "$(pwd)/sa.json:/secrets/sa.json:ro" \
  -v "$(pwd)/config:/app/config" \
  -v "$(pwd)/data:/app/data" \
  sustentabilitas/synapse-gateway
```

On a Mac with Apple silicon, add `--platform linux/amd64` after `docker run`: the image is
built for `linux/amd64` only.

The image runs as the non-root user `synapse` (UID 1001) with no home directory, so it
cannot see your `gcloud` credentials; the mounted key takes their place.
`SYNAPSE_LEDGER_SQLITE_DSN` puts the SQLite ledger in the mounted `data/` folder, so it
survives the container and you can query it from your machine.

:::note
On Linux, UID 1001 must be able to read `sa.json` and write to `data/`. On a development
machine, the simplest fix is:

```bash
chmod a+r sa.json && chmod a+rwx data
```

If the gateway cannot open the ledger, it logs the error and keeps serving requests without
recording them.
:::

Mounting `config/` replaces the image's whole `/app/config` directory, so only the files you
provide are used.

### Cargo

From the `synapse-quickstart` directory, with the binary from `cargo install synapse-gateway`:

```bash
gcloud auth application-default login
export VERTEX_PROJECT_ID=my-gcp-project
export OPENAI_API_KEY=sk-...
SYNAPSE_ROUTES_PATH=config/routes.toml SYNAPSE_PRICING_PATH=config/pricing.toml \
  SYNAPSE_GUARDRAILS_PATH=config/guardrails.toml \
  SYNAPSE_LEDGER_SQLITE_DSN="sqlite://data/synapse.db?mode=rwc" \
  synapse-gateway
```

To run from a clone of the repository instead, replace `synapse-gateway` with
`cargo run --release -p synapse-gateway --manifest-path <path-to-clone>/Cargo.toml`. The
paths stay relative to your working directory.

Either way, the API listens on port `8080`, Prometheus metrics on port `9090`, and the cost
ledger is written to `data/synapse.db`.

:::tip
No OpenAI key? Set `SYNAPSE_PROVIDER_VALIDATION=lenient` instead of `OPENAI_API_KEY`.
Synapse then drops the `openai` leg and starts with `chat` served by Vertex alone. The
default, `strict`, refuses to start when a route references a provider without credentials.
:::

## Check that it is running

```bash
curl -s http://localhost:8080/health
```

The response is the plain-text body `ok`.

List the model aliases your clients can use:

```bash
curl -s http://localhost:8080/v1/models
```

```json
{"object":"list","data":[{"id":"chat","object":"model","owned_by":"synapse"},{"id":"gemini-flash","object":"model","owned_by":"synapse"}]}
```

## Send a standard request

```bash
curl -s http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "x-synapse-tenant: my-team" \
  -d '{
    "model": "gemini-flash",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

The response is a standard OpenAI `chat.completion` object. The `x-synapse-tenant` header
attributes the request's tokens and cost to `my-team` in the ledger; without it, requests are
attributed to `unattributed`.

Because the request is plain OpenAI, any OpenAI SDK works too: point its base URL at
`http://localhost:8080/v1` and use `gemini-flash` as the model. Synapse has no inbound
authentication, but most SDKs require an API key, so pass any non-empty string. See
[Security](../operating/security.md#callers-arent-authenticated) and
[Limitations and roadmap](../reference/limitations-roadmap.md).

## Send a streaming request

```bash
curl -sN http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "x-synapse-tenant: my-team" \
  -d '{
    "model": "gemini-flash",
    "messages": [{"role": "user", "content": "Count to 5."}],
    "stream": true
  }'
```

The response is a stream of server-sent events in the OpenAI format: `data: {...}` lines
carrying `chat.completion.chunk` objects, ending with `data: [DONE]`.

## Send a request with fallback

The `chat` route has two legs, so this step needs `OPENAI_API_KEY`. Send it the same kind of
request:

```bash
curl -s http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "x-synapse-tenant: my-team" \
  -d '{
    "model": "chat",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

The response's `model` field names the leg that answered. While Vertex AI is healthy it is
`gemini-3.5-flash-lite`. To watch the fallback, restart the gateway with
`VERTEX_PROJECT_ID` set to a project that does not exist and repeat both requests:
`gemini-flash` now fails with `502` and error code `all_legs_failed`, while `chat` answers
with `"model": "gpt-4o-mini"`. Your client sees one successful response either way.

## Send a native Vertex request

Add a `vertex` block to use Vertex-only features. This request attaches a public sample
video from Cloud Storage:

```bash
curl -s http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "x-synapse-tenant: my-team" \
  -d '{
    "model": "gemini-flash",
    "messages": [{"role": "user", "content": "Describe this video."}],
    "vertex": {
      "media_uris": ["gs://cloud-samples-data/video/animals.mp4"]
    }
  }'
```

The response is a normal `chat.completion` describing the video. The `vertex` block sends
the request to the native Vertex lane, which keeps Vertex-only fields that the standard lane
would drop. Only `vertex` legs serve it: on the `chat` route, the OpenAI leg is skipped. A
route with no `vertex` leg returns `400` with error code `native_feature_unsupported`. See
[Native Vertex lane](../overview/architecture.md#native-vertex-lane) for how it works.

### Optional: reuse a context cache

If you have created a Vertex AI context cache, add its resource name as `cached_content`:

```json
"vertex": {
  "media_uris": ["gs://cloud-samples-data/video/animals.mp4"],
  "cached_content": "projects/my-gcp-project/locations/us/cachedContents/abc123"
}
```

The cache must exist in your project for the same model and region as the route's Vertex
leg, here `gemini-3.5-flash-lite` in `us`. Without such a cache, Vertex AI rejects the
request.

## See what it cost

With the default SQLite ledger, the gateway writes one row per completed request. Query the
ledger file from your machine:

```bash
sqlite3 data/synapse.db \
  'SELECT tenant, route, provider, model, lane, input_tokens, output_tokens, cost_usd FROM usage_events;'
```

Each request you sent appears as a row attributed to `my-team`, with its lane (`standard` or
`native`) and its cost from `pricing.toml`.

## Next steps

- Follow the tutorials: [cache a video and get structured answers](tutorials/native-vertex-caching.md),
  [route requests by difficulty with Jev](tutorials/jev-tiers.md) and
  [fall back across providers](tutorials/fallback-across-providers.md).
- Add your own routes, legs and tiers with the [routes reference](../configuration/routes.md),
  and set prices with [Pricing](../configuration/pricing.md).
- Learn each feature from the guides: the [native Vertex lane](../guides/native-vertex.md),
  the [Jev router](../guides/jev-router.md), [fallback chains](../guides/fallback-chains.md),
  [streaming and tool calling](../guides/streaming-and-tools.md),
  [tenant attribution](../guides/tenant-attribution.md) and the
  [cost ledger](../guides/cost-ledger.md).
- Deploy it with [Docker](../deployment/docker.md) or
  [Docker Compose](../deployment/docker-compose.md), then work through the
  [production checklist](../deployment/production-checklist.md).
- Watch it with [Metrics](../operating/metrics.md) and the
  [Grafana dashboard](../operating/grafana-dashboard.md).
- Look up endpoints and error codes in the [HTTP API reference](../reference/http-api.md),
  settings in [Environment variables](../configuration/environment-variables.md), and what
  Synapse does not do yet in [Limitations and roadmap](../reference/limitations-roadmap.md).
- Read [Architecture](../overview/architecture.md) to see how lanes and fallback chains fit
  together.
- Look up routes, legs, tiers and tenants in [Concepts](../overview/concepts.md).
