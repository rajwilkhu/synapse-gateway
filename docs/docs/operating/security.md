---
sidebar_position: 4
title: Security
description: What the gateway does and doesn't protect, how to deploy it safely, and how to report a vulnerability.
---

The gateway holds your provider credentials and spends money on every call, but it doesn't
authenticate the callers who reach it. Treat it as an internal service: put it on a private
network behind something that decides who may call it. This page lists what the gateway leaves
to you. The [production checklist](../deployment/production-checklist.md#security) turns it
into steps.

## Report a vulnerability

Don't report security vulnerabilities in public GitHub issues, discussions or pull requests.
Report them privately, in one of two ways:

- open a draft advisory under the repository's **Security → Advisories** tab (preferred), or
- email [raj@sustentabilitas.com](mailto:raj@sustentabilitas.com).

Include the affected version or commit, a description of the issue and its impact, reproduction
steps or a proof of concept, and any relevant configuration and logs, with secrets removed. You
get an acknowledgement within three business days. The project follows coordinated disclosure:
give the maintainers a reasonable window to release a fix before you publish, and say whether
you want to be credited. Vulnerabilities in dependencies belong with those projects, but the
maintainers will help coordinate if one affects the gateway.

The full policy is in
[SECURITY.md](https://github.com/sustentabilitas/synapse-gateway/blob/main/SECURITY.md).

## Callers aren't authenticated

The gateway has no API keys, no inbound authentication and no rate limiting. Anyone who can
reach the listener can:

- call any route, and spend your provider quota and money;
- call the [Gemini passthrough](../guides/native-vertex.md#gemini-sdk-clients) with any Vertex
  AI model name, including models that no route lists. A model the route table doesn't know is
  forwarded as-is, with the gateway's Google credentials;
- call the Jev passthrough, `POST /typesafe/v1/systemone`, when `TYPESAFE_API_KEY` is set;
- register and remove A2A agents (see
  [the A2A admin endpoints](#the-a2a-admin-endpoints-are-open)).

Put an authenticating reverse proxy or API gateway in front of it, and apply rate limits and
budgets there. The gateway serves plain HTTP; terminate TLS in the same proxy or in your
service mesh.

## Tenant headers are trusted

The [attribution headers](../guides/tenant-attribution.md#attribution-headers), such as
`x-synapse-tenant` and `x-synapse-user`, go straight into the cost ledger. The gateway doesn't
check them, so a caller can charge its usage to another tenant by sending that tenant's name.
If you bill or set budgets from the ledger, have your proxy remove any attribution headers the
caller sent and set them from the caller's authenticated identity.

## The A2A admin endpoints are open

The gateway serves an A2A agent catalogue. Two of its endpoints change the catalogue and have
no authentication:

- `POST /internal/a2a/agents` registers an agent;
- `DELETE /internal/a2a/agents/{id}` removes one.

By default they listen on `127.0.0.1:8081`, apart from the API port. On a host, only local
processes can reach them. In Docker or Kubernetes, `127.0.0.1` is the container's own
loopback, so nothing outside the container can reach them. To let agent hosts register, set
`SYNAPSE_ADMIN_ADDR=0.0.0.0:8081` and publish or expose that port only to them, or set
`SYNAPSE_ADMIN_ADDR=shared` to serve the endpoints on the API port.

Registration never overwrites an existing id, but a caller can delete an agent and register a
new one under the same id with its own endpoint. Clients that resolve agents through the
catalogue would then be sent to that endpoint. Block `/internal/` at your proxy for everything
except the services that register agents, or seed the catalogue from `a2a.toml` at startup and
block the admin endpoints entirely.

## The metrics port is unauthenticated

The metrics listener, `SYNAPSE_METRICS_ADDR` (default `0.0.0.0:9090`), serves `/metrics` to
anyone who can reach it. The series name your routes, upstream models, guardrail policies and
Jev tiers. Expose the port only to your Prometheus.

## Credentials

The gateway reads provider credentials from environment variables and from the service-account
file that `GOOGLE_APPLICATION_CREDENTIALS` points to.
[Environment variables](../configuration/environment-variables.md#providers) lists them all.

- Inject them from a secret store, not from files baked into an image or committed to a
  repository.
- Prefer a workload identity for Vertex AI over a service-account key file: Application Default
  Credentials picks it up with no key to leak.
- Give each credential only the access the gateway needs. The Gemini passthrough can reach any
  Vertex AI model the Google identity may call, so the identity's permissions are the real
  limit on what callers can use.
- The ledger DSNs contain database passwords. Keep them in the secret store as well.

The gateway never logs credentials, and it doesn't return provider credentials to callers.

## Content

The gateway doesn't log prompts or completions, and the cost ledger stores no content: each row
holds identifiers, the route, provider and model, token counts, cost and status. See
[What is logged](./logging.md#what-is-logged) and the
[row format](../guides/cost-ledger.md#row-format).

Provider error bodies are different: when every leg fails, the gateway returns each leg's error
to the caller, and a provider's error message can quote part of the request.

## Guardrails

[Guardrail policies](../configuration/guardrails-policy.md) scan the text of every lane (chat,
Gemini passthrough, embeddings and Jev passthrough) before any provider sees it, under the
route's `policy`, else `default`; see [What is
scanned](../configuration/guardrails-policy.md#what-is-scanned). A policy in `block` mode stops
a matching request with a `400` that names the policy and the scanners that matched; see [Block
response](../configuration/guardrails-policy.md#block-response). Policies are a
pattern-matching first line of defence, not a complete filter:

- they scan responses only for chat completions under a policy with
  [`output_scanners`](../configuration/guardrails-policy.md#output-scanning); passthrough,
  embeddings and Jev responses are never scanned;
- a route without a `policy` uses the policy named `default`, and has no protection if you
  don't define one;
- the block response tells the caller which scanners matched, which helps an attacker adjust a
  prompt until it passes.

## Request size

Request bodies are parsed as JSON, with axum's default limit of 2 MB. Larger bodies are
rejected before they reach a provider. The limit isn't configurable, so send large media to
Vertex AI by reference (for example with `vertex.media_uris` on the native lane) rather than
inline.
