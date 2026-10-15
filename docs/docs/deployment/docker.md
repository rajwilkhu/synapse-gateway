---
sidebar_position: 1
title: Docker
description: Run the Synapse gateway and proxy images, with your own configuration, credentials and ledger backend.
---

You can run Synapse in any container runtime from the images on Docker Hub. This page covers
what the images contain and how to supply configuration, credentials and a ledger backend.
To try the gateway for the first time, follow the [quickstart](../get-started/quickstart.md)
instead; to run it next to Postgres and Prometheus, see [Docker Compose](docker-compose.md).

## Images

| Image | Binary | Ports | Runs as |
|---|---|---|---|
| `sustentabilitas/synapse-gateway` | `synapse-gateway` | `8080` API, `9090` Prometheus metrics, `8081` A2A admin (container loopback) | `synapse` (UID 1001) |
| `sustentabilitas/synapse-proxy` | `synapse-proxy` | `8787` proxy traffic, `9090` Prometheus metrics | `synapse` (UID 1001) |

Each release is tagged with its version (for example `sustentabilitas/synapse-gateway:2.0.0`)
and as `latest`. Every push to `main` is published as `edge`. The images are built for
`linux/amd64` only. On an ARM machine, such as a Mac with Apple silicon, add
`--platform linux/amd64` to `docker pull` and `docker run` (or `platform: linux/amd64` in
Compose) and Docker runs them under emulation; without it, the pull fails with
`no matching manifest for linux/arm64/v8`.

Both images are built from `debian:bookworm-slim`. The user `synapse` has no home directory
and no login shell, and the working directory is `/app`.

## Run the gateway

A typical gateway container mounts a configuration directory and a Google Cloud
service-account key, and reads provider keys from an environment file:

```bash
docker run -d --name synapse \
  -p 8080:8080 -p 9090:9090 \
  --env-file synapse.env \
  -e GOOGLE_APPLICATION_CREDENTIALS=/secrets/sa.json \
  -v "$(pwd)/sa.json:/secrets/sa.json:ro" \
  -v "$(pwd)/config:/app/config:ro" \
  sustentabilitas/synapse-gateway:2.0.0
```

with `synapse.env` holding, for example:

```bash
VERTEX_PROJECT_ID=my-gcp-project
OPENAI_API_KEY=sk-...
SYNAPSE_DEFAULT_TENANT=my-deployment
```

Check that it started:

```bash
curl -s http://localhost:8080/health
```

`GET /health` returns the plain-text body `ok` as soon as the API listener is up. Use it for
container health checks and load balancer probes. It does not call any provider.

### Configuration

The gateway reads its TOML files from `/app/config`:

| File | Required | Reference |
|---|---|---|
| `routes.toml` | Yes | [Routes](../configuration/routes.md) |
| `pricing.toml` | Yes | [Pricing](../configuration/pricing.md) |
| `guardrails.toml` | No | [Guardrails policy](../configuration/guardrails-policy.md) |
| `ai_task_types.toml` | No | [AI task types](../guides/tenant-attribution.md#ai-task-types) |
| `a2a.toml` | No | [Configuration keys](../reference/configuration-keys.md#a2atoml) (seed for the A2A agent registry) |

The image ships with the example configuration from
[`crates/synapse-gateway/config/`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/config),
which references providers you may not have credentials for. Always mount your own
directory over `/app/config`. The mount replaces the whole directory, so only the files you
provide are used: leave out `guardrails.toml` and the gateway refuses to start, unless
`SYNAPSE_GUARDRAILS=optional`, which turns guardrails off.

To keep the files somewhere else in the container, set `SYNAPSE_ROUTES_PATH`,
`SYNAPSE_PRICING_PATH` and the other path variables listed in
[Environment variables](../configuration/environment-variables.md#configuration-files).

The configuration files and the service-account key must be readable by UID 1001. The gateway never writes to its configuration
directory, so a read-only mount (`:ro`) works.

### Credentials

Each provider reads its credentials from environment variables; see
[Providers](../configuration/providers.md). Pass secrets with `--env-file` or your
orchestrator's secret store rather than on the command line, where they end up in your shell
history and in `docker inspect`.

Vertex AI uses Google Application Default Credentials. Because the container user has no
home directory, credentials from `gcloud auth application-default login` on your machine are
not visible inside the container. Either:

- mount a service-account key and set `GOOGLE_APPLICATION_CREDENTIALS` to its path, as
  above; or
- run on Google Cloud (Cloud Run, GKE with Workload Identity, Compute Engine), where the
  metadata server provides credentials and you need neither.

The account needs permission to call Vertex AI, for example the Vertex AI User role.

With the default strict provider validation, the gateway refuses to start when a route uses a
provider whose credential variable is unset. It only checks that the variables are set;
invalid keys show up as failed requests. See
[Strict and lenient validation](../configuration/providers.md#strict-and-lenient-validation).

### Ledger

`SYNAPSE_LEDGER_BACKENDS` selects where usage rows go. The image is built with every
backend, so any combination of `sqlite`, `postgres`, `pubsub` and `sns` works without
rebuilding:

```bash
docker run -d --name synapse \
  -p 8080:8080 -p 9090:9090 \
  --env-file synapse.env \
  -e SYNAPSE_LEDGER_BACKENDS=postgres \
  -e SYNAPSE_LEDGER_POSTGRES_DSN=postgres://synapse:change-me@db.internal:5432/synapse \
  -v "$(pwd)/config:/app/config:ro" \
  sustentabilitas/synapse-gateway:2.0.0
```

The gateway creates the `usage_events` table itself on startup; there are no migrations to
run.

The default, `sqlite`, writes `synapse.db` into `/app`, inside the container, so the ledger
disappears with the container. To keep it, mount a volume and point
`SYNAPSE_LEDGER_SQLITE_DSN` at it, as the quickstart does with
`sqlite:///app/data/synapse.db?mode=rwc`. The directory must be writable by UID 1001.

A sink that cannot connect at startup, such as a Postgres server that is not up yet, is
logged and skipped, and the gateway serves requests without recording them to that sink.
Start the database first, and check the logs for `ledger sink connected`. The
[cost ledger guide](../guides/cost-ledger.md) covers what each sink receives, and
[Ledger](../configuration/environment-variables.md#ledger) lists every connection variable.

### Ports

`SYNAPSE_ADDR` (default `0.0.0.0:8080`) and `SYNAPSE_METRICS_ADDR` (default `0.0.0.0:9090`)
set the listen addresses inside the container. The metrics port serves only Prometheus
metrics, at `/metrics` and `/`; every other endpoint, including `/health`, is on the API
port. Publish the metrics port only to your monitoring network.

`SYNAPSE_ADMIN_ADDR` (default `127.0.0.1:8081`) serves the A2A admin endpoints. Inside a
container, `127.0.0.1` is the container's own loopback, so nothing outside it can reach
them. To let agent hosts register, set `SYNAPSE_ADMIN_ADDR=0.0.0.0:8081` and publish that
port only to them, or set `SYNAPSE_ADMIN_ADDR=shared` to serve the endpoints on the API port.
See [Security](../operating/security.md#the-a2a-admin-endpoints-are-open).

### Stopping

The gateway installs no signal handlers, and the image has no init process. Linux does not
deliver `SIGTERM` to a container's process 1 unless it handles the signal, so `docker stop`
can wait out its grace period (10 seconds by default) before killing the gateway. Add
`--init` to `docker run`, or `init: true` in Compose, so the gateway exits as soon as it is
signalled.

Either way the gateway does not drain: in-flight requests are cut off, and ledger rows still
queued in memory are lost. Stop sending traffic to an instance before you stop it.

## Build the image

Build either image from the repository root:

```bash
docker build -f crates/synapse-gateway/Dockerfile -t synapse-gateway .
docker build -f crates/synapse-proxy/Dockerfile -t synapse-proxy .
```

The gateway build enables the `ledger-postgres`, `ledger-pubsub` and `ledger-sns` features
on top of the defaults, and installs `cmake` and `protobuf-compiler` for them. To build a
smaller image with fewer ledger backends, change the `--features` list in
[`crates/synapse-gateway/Dockerfile`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/Dockerfile).

## Run the proxy

`synapse-proxy` is a separate reverse-proxy sidecar, not part of the gateway. Its image reads
`/app/synapse-proxy.toml`, an example you replace by mounting your own file or by setting
`SYNAPSE_PROXY_CONFIG_PATH`:

```bash
docker run -d --name synapse-proxy \
  -p 8787:8787 -p 9090:9090 \
  -v "$(pwd)/synapse-proxy.toml:/app/synapse-proxy.toml:ro" \
  sustentabilitas/synapse-proxy:1.0.0
```

The proxy listens on the addresses in its configuration file: `addr` (`0.0.0.0:8787` in the
example) for traffic, `admin_addr` (`127.0.0.1:8788`) for its admin API and `metrics_addr`
(`0.0.0.0:9090`) for metrics. `SYNAPSE_PROXY_ADDR` overrides `addr`. With the admin API on
`127.0.0.1`, only processes inside the container, or in the same pod, can reach it. Unlike
the gateway, the proxy drains on `SIGTERM`: its readiness probe, `GET /healthz/readiness`,
starts returning `503`, and about a second later it stops accepting connections and waits
for in-flight requests to finish.
