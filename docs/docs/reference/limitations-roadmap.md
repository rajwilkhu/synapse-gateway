---
sidebar_position: 4
title: Limitations and roadmap
description: What the gateway and its sibling crates don't do yet, and the gaps to plan around.
---

This page collects what Synapse doesn't do, so you can plan around it before you depend on it.
Each item links to the page that covers it in detail.

## Planned

These are not in the current release and are planned for future releases:

- **Inbound authentication.** There are no API keys and no caller authentication. Run the
  gateway behind an API gateway, ingress or service mesh that authenticates callers; see
  [Security](../operating/security.md#callers-arent-authenticated).
- **Rate limiting.** Nothing limits how fast a caller can spend your quota. Apply limits in
  front of the gateway.
- **Dynamic route reloading.** There is no admin API for changing routes. Every configuration
  file is read once at startup, so a change needs a restart; see
  [Configuration keys](./configuration-keys.md).

## Request fields

The gateway accepts any OpenAI chat field but forwards only some of them, and drops the rest
without an error:

- On the **standard lane**, `max_tokens`, `tool_choice`, `top_p`, `stop`, `seed` and every
  other field not listed in [`POST /v1/chat/completions`](./http-api.md#post-v1chatcompletions)
  are dropped. A response can therefore be longer than the `max_tokens` you sent. See
  [Tool calling](../guides/streaming-and-tools.md#tool-calling) for `tool_choice`.
- On the **native Vertex lane**, only `temperature`, `max_tokens`, `tools`, `tool_choice` and
  the `vertex` block are forwarded. `response_format` is ignored (use
  `vertex.response_schema`), and `system` messages are sent as user turns. See
  [Other request fields](../guides/native-vertex.md#other-request-fields).
- Streamed chunks carry no `usage`. Token counts go to the ledger and the metrics instead.
- Request bodies are limited to 2 MB. Send large media to Vertex AI by Cloud Storage URI.

## Resilience

- **Retries and circuit breakers are standard-lane only.** Each `(provider, model)` leg on the
  standard lane has its own retry policy and circuit breaker, and records
  `synapse_resilience_*` metrics and `synapse::resilience` logs. On the native Vertex, Jev and
  embedding lanes, each leg gets one attempt per request with no circuit breaker, and the next
  leg in the chain provides failover. See
  [Fallback chains](../guides/fallback-chains.md#the-order-of-legs).

## Timeouts

- **The native Vertex lane has no first-chunk or idle timeout.** Only
  `SYNAPSE_REQUEST_TIMEOUT_SECS` bounds a call, and it covers the whole response, so a slow
  first token waits the full timeout before the next `vertex` leg is tried. See
  [Timeouts](../guides/native-vertex.md#timeouts).
- **Streaming on the standard lane has no idle timeout** once the first chunk has arrived.
- **`SYNAPSE_REQUEST_TIMEOUT_SECS` covers the whole response** on every lane, so long
  generations fail mid-stream unless you raise it. See
  [Streaming and tool calling](../guides/streaming-and-tools.md#timeouts).

## Observability

- **No tracing.** The gateway records OpenTelemetry metrics only; it emits no OpenTelemetry
  spans and doesn't propagate trace context to providers. See
  [Logging](../operating/logging.md#tracing).
- **No metric for failed chat completions.** The request metrics count only requests that
  produced a response. Measure error rates in front of the gateway; see
  [What the request metrics count](../operating/metrics.md#what-the-request-metrics-count).
- **Plain-text logs only.** There is no JSON log format and no access log. See
  [Logging](../operating/logging.md).
- **`synapse-proxy` exposition quirks.** The proxy's counters have a doubled `_total` suffix
  and its duration histogram has millisecond-sized buckets. See
  [synapse-proxy metrics](./metrics-catalogue.md#synapse-proxy).

## Cost ledger accuracy

The ledger's costs are estimates built from provider token counts and your `pricing.toml`.
Cached native Vertex input is priced at the full rate, native Vertex thinking tokens are not
counted, failed attempts and abandoned streams record no tokens, unpriced chat models cost 0,
and rows can be dropped when the queue is full. [Accuracy](../guides/cost-ledger.md#accuracy)
explains each gap.

## Operations

- **No graceful shutdown.** The gateway doesn't drain on `SIGTERM`: in-flight requests are cut
  off and ledger rows still in the queue are lost. See
  [Stopping](../deployment/docker.md#stopping).
- **The A2A registry is in memory, per instance.** Agents registered through the admin API
  exist only on the instance that received the call, and are lost on restart. Seed shared
  agents from `a2a.toml`; see
  [Running more than one instance](../deployment/production-checklist.md#running-more-than-one-instance).
- **SQLite is single-node.** Several instances need a shared sink such as Postgres; see
  [Sinks](../guides/cost-ledger.md#sinks).
- **The Docker images are `linux/amd64` only.** On ARM hosts, run them under emulation with
  `--platform linux/amd64`; see [Docker](../deployment/docker.md).

## synapse-mcp

The MCP gateway crate routes each MCP server separately. Not yet supported:

- **Tool aggregation across servers.** There is no merged tool surface; each server is reached
  at its own `/mcp/<server>` path.
- **SSE back-compat** for upstream servers that speak only the legacy SSE transport.
- **Several concurrent identity bindings.** One identity overlay is active at a time, matching
  `synapse-context`'s `ContextStore`.
