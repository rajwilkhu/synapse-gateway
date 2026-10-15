---
sidebar_position: 1
title: Installation
description: Install the Synapse gateway as a Docker image, as a binary with Cargo, or as a library crate in your Rust service.
---

You can run Synapse from the published Docker image, install the binary with Cargo, or
embed the gateway in your own Rust service as a library. The image is the quickest way to
try it; the library suits services that want routing and cost accounting in-process.

## Docker

Pull the image from Docker Hub:

```bash
docker pull sustentabilitas/synapse-gateway
```

Tags are published for every release (for example `2.0.0`), as `latest` for the most
recent release, and as `edge` for the tip of `main`. Images are built for `linux/amd64`
only. On Apple silicon or another ARM machine, add `--platform linux/amd64` to `docker pull`
and `docker run`, and Docker runs them under emulation.

The image:

- listens on port `8080` for the API and port `9090` for Prometheus metrics, and serves the
  A2A admin endpoints on `127.0.0.1:8081`, the container's own loopback (see
  [Docker](../deployment/docker.md#ports));
- runs as the non-root user `synapse` (UID 1001), which has no home directory;
- uses `/app` as its working directory and reads configuration from `/app/config`;
- includes every ledger backend (SQLite, Postgres, Pub/Sub and SNS).

The image ships with the default configuration from
[`crates/synapse-gateway/config/`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/config).
Mount your own directory over `/app/config` to supply your routes, pricing and guardrails:

```bash
docker run --rm -p 8080:8080 -p 9090:9090 \
  -v "$(pwd)/config:/app/config" \
  sustentabilitas/synapse-gateway
```

Vertex AI legs authenticate with Google Application Default Credentials. The container user
has no home directory, so credentials from `gcloud auth application-default login` are not
available inside it. Mount a service-account key and set `GOOGLE_APPLICATION_CREDENTIALS`,
or run on Google Cloud, where the metadata server provides credentials. The
[quickstart](quickstart.md) shows the full command.

To build the image yourself from the repository root:

```bash
docker build -f crates/synapse-gateway/Dockerfile -t synapse-gateway .
```

## Cargo

Install the `synapse-gateway` binary from crates.io:

```bash
cargo install synapse-gateway
```

Or build it from source:

```bash
git clone https://github.com/sustentabilitas/synapse-gateway.git
cd synapse-gateway
cargo build --release -p synapse-gateway
```

The binary is written to `target/release/synapse-gateway`. It reads `config/routes.toml`
and `config/pricing.toml` relative to the working directory; set `SYNAPSE_ROUTES_PATH` and
`SYNAPSE_PRICING_PATH` to use other paths.

Cargo features select the HTTP server and the ledger backends:

| Feature | Default | Enables |
|---|---|---|
| `server` | Yes | The HTTP server and the `synapse-gateway` binary. |
| `ledger-sqlite` | Yes | The SQLite ledger backend. |
| `ledger-postgres` | No | The Postgres ledger backend. |
| `ledger-pubsub` | No | Publishing ledger events to Google Cloud Pub/Sub. |
| `ledger-sns` | No | Publishing ledger events to AWS SNS. |

Features are additive over the defaults. For example:

```bash
# Default: server + SQLite ledger
cargo install synapse-gateway

# Add the Postgres ledger and Pub/Sub publishing
cargo install synapse-gateway --features "ledger-postgres ledger-pubsub"

# Postgres ledger only, no SQLite
cargo install synapse-gateway --no-default-features --features "server ledger-postgres"
```

:::note
The cloud ledger features pull in larger dependency trees. The Docker build installs
`cmake` and `protobuf-compiler` for them; install both if a build with `ledger-pubsub` or
`ledger-sns` fails.
:::

## As a library

Add the crate without its default features, so you get the gateway without the HTTP server
or a ledger backend:

```toml
[dependencies]
synapse-gateway = { version = "2", default-features = false }
```

The library crate is named `synapse`, so you import it as `synapse::…`, for example
`use synapse::gateway::Gateway;`. Build a gateway with `Gateway::builder()` and call
`Gateway::chat()` in-process. Add `ledger-sqlite`, `ledger-postgres`, `ledger-pubsub` or
`ledger-sns` to `features` if you want the corresponding ledger backend.

[Embedding Synapse as a library](../guides/embedding-as-library.md) walks through a complete
example, including streaming, embeddings and what the builder leaves out.
[Workspace crates](../internals/workspace-crates.md#synapse-gateway) describes the crate's
modules and features.

The API reference is on [docs.rs](https://docs.rs/synapse-gateway).
