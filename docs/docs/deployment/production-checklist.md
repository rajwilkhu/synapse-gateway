---
sidebar_position: 3
title: Production checklist
description: What to decide and configure before the Synapse gateway serves real traffic, from image tags and credentials to the ledger, metrics and inbound authentication.
---

Work through this list before the gateway serves real traffic. Each item links to the page
that explains the setting in full.

## Image and configuration

- [ ] **Pin the image tag.** Run a release tag such as
  `sustentabilitas/synapse-gateway:2.0.0`, not `latest` or `edge`, so a restart never
  changes the version you run. Read the changelog before you move the tag. See
  [Docker](docker.md#images).
- [ ] **Mount your own `/app/config`.** The image's built-in configuration is an example.
  Keep your `routes.toml`, `pricing.toml` and optional files in version control and mount
  them read-only. The gateway reads them once at startup and has no reload endpoint, so a
  change needs a restart. See [Docker](docker.md#configuration).
- [ ] **Keep strict provider validation.** Leave `SYNAPSE_PROVIDER_VALIDATION` unset or
  `strict`, so a missing credential stops the gateway at startup instead of silently dropping
  legs from your routes. See
  [Strict and lenient validation](../configuration/providers.md#strict-and-lenient-validation).
- [ ] **Test every leg.** A misconfigured leg on the standard lane fails over silently on
  every request, adding latency. Call each provider and model through a single-leg route
  before relying on a chain. See [Fallback chains](../guides/fallback-chains.md#design-a-chain).
- [ ] **Keep prices current.** A chat model missing from `pricing.toml` is recorded at cost 0.
  See [Pricing](../configuration/pricing.md).

## Credentials

- [ ] **Store provider keys as secrets.** Pass them from your orchestrator's secret store or
  an `--env-file`, never baked into an image or written on a command line.
- [ ] **Give Vertex AI its own identity.** Use Workload Identity or the metadata server on
  Google Cloud, or a service-account key limited to calling Vertex AI elsewhere. See
  [Credentials](docker.md#credentials).

## Timeouts

- [ ] **Size `SYNAPSE_REQUEST_TIMEOUT_SECS` for your longest response.** It bounds the first
  chunk on the standard lane and, as the provider HTTP timeout, every whole response, on
  every lane. Long structured outputs or heavy thinking need more than the default 120
  seconds. See [Timeouts](../guides/streaming-and-tools.md#timeouts).
- [ ] **Account for the chain.** Each leg can use the full timeout before the next starts, so
  a three-leg chain can take three times as long to fail. Set your load balancer and client
  timeouts above that.

## Ledger

- [ ] **Choose a ledger backend.** The default SQLite ledger is a file on one machine, inside
  the container unless you mount a volume. It suits a single instance. With more than one
  replica, use `postgres`, and add `pubsub` or `sns` for usage you must not lose. See
  [Cost ledger](../guides/cost-ledger.md#sinks).
- [ ] **Check that every sink connected.** A sink that fails at startup is skipped and the
  gateway keeps serving without it. Look for `ledger sink connected` in the startup logs for
  each backend.
- [ ] **Alert on lost rows.** `synapse_ledger_dropped_total` counts rows dropped because the
  queue was full; `synapse_ledger_errors_total` counts failed writes. Both should stay at 0.
  See [Delivery](../guides/cost-ledger.md#delivery).

## Observability

- [ ] **Collect metrics.** Scrape `:9090` with Prometheus, or set
  `OTEL_EXPORTER_OTLP_ENDPOINT` so the gateway pushes metrics to your collector. Keep the
  metrics port off the public network. See
  [Environment variables](../configuration/environment-variables.md#telemetry).
- [ ] **Measure errors outside the gateway.** Synapse's request metrics count requests that
  produced a response; requests that fail with a `4xx` or `5xx` are not counted. Take error
  rates from your load balancer, ingress or service mesh.
- [ ] **Ship logs.** The gateway logs to standard output in plain text. Set `RUST_LOG=info`
  (the default) and `NO_COLOR=1` so your log collector gets text without colour codes.

## Security

- [ ] **Put an authenticating proxy in front.** Synapse has no inbound authentication or
  rate limiting: anyone who can reach port `8080` can spend your provider budget. Run it
  behind an API gateway, ingress or service mesh that authenticates callers and limits their
  request rate.
- [ ] **Set the tenant at the proxy.** Clients can send any `x-synapse-tenant`. If
  attribution matters, have your proxy set the header from the authenticated identity,
  overwriting the client's value. See [Tenant attribution](../guides/tenant-attribution.md).
- [ ] **Block the internal endpoints.** The A2A registry's admin endpoints, `POST
  /internal/a2a/agents` and `DELETE /internal/a2a/agents/{id}`, are served on
  `SYNAPSE_ADMIN_ADDR`. Keep it unset (loopback) or reachable only from agent hosts; avoid
  `shared`.
- [ ] **Guard every lane.** Define a `default` policy or a policy per route and embedding
  alias. Keep `SYNAPSE_GUARDRAILS=required` and `SYNAPSE_PASSTHROUGH_MODELS=routed`.
- [ ] **Configure guardrails.** Add a `guardrails.toml` with a `default` policy, rolled out in
  `observe` mode first, so prompt injection, secrets and personal data are caught before they
  reach a provider. See [Guardrails policy](../configuration/guardrails-policy.md).
- [ ] **Set `SYNAPSE_DEFAULT_TENANT`.** Give requests without a tenant header a recognisable
  tenant, such as the deployment's name, instead of `unattributed`, so unattributed spend
  stands out in the ledger.

## Running more than one instance

- [ ] **Share the ledger.** Every replica writes its own rows; point them all at the same
  Postgres database or message topic.
- [ ] **Seed the A2A registry from a file.** The registry is in memory, per instance: an agent
  registered through `POST /internal/a2a/agents` exists only on the replica that received the
  call, and every registration is lost on restart. Seed agents from `a2a.toml` on every
  replica instead.
- [ ] **Drain before stopping.** The gateway does not finish in-flight requests on shutdown.
  Remove an instance from your load balancer before you stop it, and use an init process so
  it exits promptly. See [Stopping](docker.md#stopping).
- [ ] **Probe `/health`.** It returns `200` with the body `ok` once the API listener is up.
