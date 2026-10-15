---
sidebar_position: 1
title: Environment variables
description: Every environment variable the Synapse gateway reads, with its default and what it controls.
---

You configure the Synapse gateway binary with environment variables, plus the TOML files
whose paths some of them set. The gateway reads them once at startup, so restart it after a
change.

## Server

| Variable | Default | Description |
|---|---|---|
| `SYNAPSE_ADDR` | `0.0.0.0:8080` | Address and port of the API: every HTTP endpoint except metrics. |
| `SYNAPSE_ADMIN_ADDR` | `127.0.0.1:8081` | Address and port of the A2A admin endpoints, `/internal/a2a/*`. `shared` serves them on `SYNAPSE_ADDR` instead. The admin endpoints have no authentication: keep this address private. |
| `SYNAPSE_METRICS_ADDR` | `0.0.0.0:9090` | Address and port of the Prometheus endpoint, `GET /metrics`. Must be an IP address and port; a host name fails at startup. |

## Configuration files

Relative paths resolve against the gateway's working directory. In the Docker image that is
`/app`, so the defaults point at the files in `/app/config`.

| Variable | Default | Description |
|---|---|---|
| `SYNAPSE_ROUTES_PATH` | `config/routes.toml` | Route aliases, their legs and tiers. Required: the gateway does not start without it. See [Routes](routes.md). |
| `SYNAPSE_PRICING_PATH` | `config/pricing.toml` | Prices per `provider:model`, used to cost ledger rows. Required. See [Pricing](pricing.md). |
| `SYNAPSE_GUARDRAILS_PATH` | `config/guardrails.toml` | Guardrail policies. Required unless `SYNAPSE_GUARDRAILS=optional`. See [Guardrails policy](guardrails-policy.md). |
| `SYNAPSE_AI_TASK_TYPES_PATH` | `config/ai_task_types.toml` | Maps route aliases to the AI task type recorded on ledger rows. Optional: without the file, every request records `simple`. See [AI task types](../guides/tenant-attribution.md#ai-task-types). |
| `SYNAPSE_A2A_PATH` | `config/a2a.toml` | Seed file for the in-memory A2A agent registry served on the API port (public routes) and `SYNAPSE_ADMIN_ADDR` (admin routes). Optional: without the file, the registry starts empty. |

## Providers

| Variable | Default | Description |
|---|---|---|
| `SYNAPSE_PROVIDER_VALIDATION` | `strict` | What to do when a route references a provider this process cannot build, because its credential is unset or the id is unknown. `strict` refuses to start. `lenient` drops those legs, removes routes left with no legs, and starts. Any value other than `lenient` means `strict`. |
| `SYNAPSE_GUARDRAILS` | `required` | `required` refuses to start when the guardrails file is missing, or when a route or embedding alias names a policy the file does not define. `optional` starts with guardrails off when the file is missing, and logs a warning for each undefined policy. Any value other than `optional` means `required`. |
| `SYNAPSE_PASSTHROUGH_MODELS` | `routed` | `routed` makes the Gemini passthrough forward only a route alias or a model that is a `vertex` leg of a route; anything else gets `404`. `any` forwards unknown models as a single leg. Any value other than `any` means `routed`. |

Each provider reads its own credential and endpoint variables. The table lists them; see
[Providers](providers.md) for which are required and how validation treats them.

| Variable | Default | Used by |
|---|---|---|
| `VERTEX_PROJECT_ID` | — | `vertex`: the Google Cloud project to call. |
| `VERTEX_PROJECT` | — | `vertex`: legacy spelling, read only when `VERTEX_PROJECT_ID` is unset. |
| `VERTEX_LOCATION` | `global` | `vertex`: location for native Vertex legs without a `region`, and for Vertex embeddings. |
| `GOOGLE_APPLICATION_CREDENTIALS` | — | `vertex`: path to a service-account key, read by Application Default Credentials. |
| `OPENAI_API_KEY` | — | `openai`: API key. |
| `OPENAI_BASE_URL` | `https://api.openai.com/v1` | `openai`: API base URL. |
| `DASHSCOPE_API_KEY` | — | `qwen`: DashScope API key. |
| `DASHSCOPE_BASE_URL` | `https://dashscope-intl.aliyuncs.com/compatible-mode/v1` | `qwen`: API base URL. |
| `OAI_COMPAT_BASE_URL` | — | `oai_compat`: base URL of your OpenAI-compatible server. |
| `OAI_COMPAT_API_KEY` | — | `oai_compat`: API key, if your server needs one. |
| `TYPESAFE_API_KEY` | — | `typesafe` legs, `strategy = "jev"` routes and the `/typesafe/v1/systemone` passthrough. |
| `TYPESAFE_BASE_URL` | `https://api.typesafe.ai` | TypeSafe API base URL, for self-hosted deployments and tests. |

## Streaming and timeouts

| Variable | Default | Description |
|---|---|---|
| `SYNAPSE_REQUEST_TIMEOUT_SECS` | `120` | Maximum time, in seconds, for a leg to produce its first chunk on the standard lane. A leg that misses it is abandoned and the next leg is tried. The same value is the HTTP timeout of every provider call, covering the whole response. |
| `SYNAPSE_STREAM_IDLE_TIMEOUT_SECS` | `60` | Maximum gap, in seconds, between two chunks for non-streaming requests on the standard lane. A leg that stalls this long fails, and the next leg is tried. Streaming requests have no idle timeout once their first chunk arrives. See [Timeouts](../guides/streaming-and-tools.md#timeouts). |

Both values must be whole numbers; anything else stops the gateway at startup. The native
Vertex lane has no first-chunk or idle timeout: only the provider HTTP timeout bounds it.

## Ledger

| Variable | Default | Description |
|---|---|---|
| `SYNAPSE_LEDGER_BACKENDS` | `sqlite` | Comma-separated ledger sinks: `sqlite`, `postgres`, `pubsub`, `sns`. Every usage event goes to every sink. An unknown or repeated name stops the gateway at startup. |
| `SYNAPSE_LEDGER_BACKEND` | — | A single sink, read only when `SYNAPSE_LEDGER_BACKENDS` is unset. |
| `SYNAPSE_LEDGER_SQLITE_DSN` | `sqlite://synapse.db?mode=rwc` | SQLite database. Falls back to `SYNAPSE_LEDGER_DSN`, then the default, a `synapse.db` file in the working directory. |
| `SYNAPSE_LEDGER_POSTGRES_DSN` | — | Postgres connection string. Falls back to `SYNAPSE_LEDGER_DSN`. Required for the `postgres` sink. |
| `SYNAPSE_LEDGER_DSN` | — | Legacy DSN shared by the SQLite and Postgres sinks. Prefer the per-sink variables. |
| `SYNAPSE_LEDGER_PUBSUB_TOPIC` | — | Google Cloud Pub/Sub topic ID. Required for the `pubsub` sink. |
| `SYNAPSE_LEDGER_PUBSUB_PROJECT` | — | Project of the Pub/Sub topic. Falls back to `VERTEX_PROJECT_ID`, then `VERTEX_PROJECT`. |
| `SYNAPSE_LEDGER_SNS_TOPIC_ARN` | — | AWS SNS topic ARN. Required for the `sns` sink. |
| `SYNAPSE_LEDGER_SNS_REGION` | — | AWS region of the topic. Without it, the AWS SDK's default region chain applies. |
| `SYNAPSE_EMBED_DEFAULT_INPUT_PRICE_PER_MTOK` | `0.10` | USD per 1,000,000 input tokens for embedding models without an entry in `pricing.toml`. An unparseable value means `0.10`. |

The `postgres`, `pubsub` and `sns` sinks need the Cargo features `ledger-postgres`,
`ledger-pubsub` and `ledger-sns`; the Docker image includes all of them. A sink whose
feature is missing, or that cannot connect at startup, is logged and skipped. If no sink
connects, the gateway still starts and serves requests without recording usage. See the
[cost ledger guide](../guides/cost-ledger.md) for what each sink receives and how reliable
delivery is.

## Tenancy

| Variable | Default | Description |
|---|---|---|
| `SYNAPSE_DEFAULT_TENANT` | `unattributed` | Tenant recorded for requests without an `x-synapse-tenant` header. |

See [Tenant attribution](../guides/tenant-attribution.md) for the attribution headers.

## Telemetry

| Variable | Default | Description |
|---|---|---|
| `OTEL_EXPORTER_OTLP_ENDPOINT` | — | Base URL of an OpenTelemetry collector, such as `http://otel-collector:4318`. When set, metrics are also pushed over OTLP/HTTP to `<endpoint>/v1/metrics`. `OTEL_EXPORTER_OTLP_METRICS_ENDPOINT` is not read. |
| `OTEL_SERVICE_NAME` | `synapse-gateway` | `service.name` resource attribute on exported metrics. |
| `OTEL_METRIC_EXPORT_INTERVAL` | `60000` | OTLP push interval in milliseconds, read by the OpenTelemetry SDK. |
| `RUST_LOG` | `info` | Log filter, for example `info` or `synapse=debug`. |

Prometheus scraping on `SYNAPSE_METRICS_ADDR` works whether or not OTLP is configured.
