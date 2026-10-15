# Synapse

[![synapse-gateway crates.io](https://img.shields.io/crates/v/synapse-gateway.svg?label=gateway)](https://crates.io/crates/synapse-gateway)
[![synapse-gateway Docker](https://img.shields.io/docker/v/sustentabilitas/synapse-gateway?logo=docker&label=gateway%20docker)](https://hub.docker.com/r/sustentabilitas/synapse-gateway)
[![synapse-proxy crates.io](https://img.shields.io/crates/v/synapse-proxy.svg?label=proxy)](https://crates.io/crates/synapse-proxy)
[![synapse-proxy Docker](https://img.shields.io/docker/v/sustentabilitas/synapse-proxy?logo=docker&label=proxy%20docker)](https://hub.docker.com/r/sustentabilitas/synapse-proxy)
[![License: MPL-2.0](https://img.shields.io/badge/License-MPL--2.0-brightgreen.svg)](LICENSE)
[![CI](https://github.com/sustentabilitas/synapse-gateway/actions/workflows/ci.yml/badge.svg)](https://github.com/sustentabilitas/synapse-gateway/actions/workflows/ci.yml)

**The LLM gateway that keeps native power.** Synapse is an open-source Rust workspace: an
OpenAI-compatible LLM gateway with native Vertex AI and Jev routing lanes, plus a
reverse-proxy sidecar, an A2A agent registry and an on-demand MCP gateway.

**Documentation:** https://synapse-gateway.readthedocs.io/en/latest/ · [Español](https://synapse-gateway.readthedocs.io/en/latest/es/)

## Crates

| Crate | Description |
|-------|-------------|
| [`synapse-gateway`](crates/synapse-gateway) | OpenAI-compatible LLM gateway: fallback chains across providers, a native Vertex AI lane, Jev routing, streaming, tool calling, per-tenant cost ledger, input guardrails. |
| [`synapse-proxy`](crates/synapse-proxy) | Config-driven reverse-proxy sidecar: path-prefix routing, context injection, request/response transforms, streaming passthrough. |
| [`synapse-a2a`](crates/synapse-a2a) | In-memory agent-to-agent (A2A) registry with admin registration and public discovery, served by the gateway. |
| [`synapse-mcp`](crates/synapse-mcp) | On-demand MCP gateway library: per-server routing over Streamable HTTP with tenant identity injected from a shared context store. |
| [`synapse-context`](crates/synapse-context) | Shared context store (static base plus TTL overlay) used by `synapse-proxy` and `synapse-mcp`. |

## Quick start

Save a route, its prices and a guardrails policy in a `config/` folder:

```toml
# config/routes.toml
[routes."gemini-flash"]
legs = [
  { provider = "vertex", model = "gemini-3.5-flash-lite", region = "us" },
]
```

```toml
# config/pricing.toml
"vertex:gemini-3.5-flash-lite" = { input = 0.30, output = 2.50 }
```

```toml
# config/guardrails.toml
[guardrails.default]
scanners = ["prompt_injection", "secrets"]
```

Start the gateway with a Google Cloud service-account key (`sa.json`) that can call Vertex AI,
then send it a standard OpenAI request:

```bash
docker run --rm -p 8080:8080 -p 9090:9090 \
  -e VERTEX_PROJECT_ID=my-gcp-project \
  -e GOOGLE_APPLICATION_CREDENTIALS=/secrets/sa.json \
  -v "$(pwd)/sa.json:/secrets/sa.json:ro" \
  -v "$(pwd)/config:/app/config" \
  sustentabilitas/synapse-gateway

# In a second terminal:
curl -s http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "x-synapse-tenant: my-team" \
  -d '{"model": "gemini-flash", "messages": [{"role": "user", "content": "Hello!"}]}'
```

The [quickstart](https://synapse-gateway.readthedocs.io/en/latest/docs/get-started/quickstart/)
walks through streaming, fallback to OpenAI, native Vertex requests and the cost ledger.

## Documentation

- [What is Synapse?](https://synapse-gateway.readthedocs.io/en/latest/docs/overview/introduction/)
- [Installation](https://synapse-gateway.readthedocs.io/en/latest/docs/get-started/installation/)
- [Quickstart](https://synapse-gateway.readthedocs.io/en/latest/docs/get-started/quickstart/)
- [Configuration](https://synapse-gateway.readthedocs.io/en/latest/docs/configuration/routes/)
- [HTTP API](https://synapse-gateway.readthedocs.io/en/latest/docs/reference/http-api/)
- [synapse-proxy](https://synapse-gateway.readthedocs.io/en/latest/docs/synapse-family/proxy/overview/),
  [synapse-a2a](https://synapse-gateway.readthedocs.io/en/latest/docs/synapse-family/a2a/overview/)
  and [synapse-mcp](https://synapse-gateway.readthedocs.io/en/latest/docs/synapse-family/mcp/overview/)

## Contributing

Contributions are welcome. See **[CONTRIBUTING.md](CONTRIBUTING.md)** for how to build, test, and submit changes. Commits must be signed off under the [Developer Certificate of Origin](https://developercertificate.org/) (`git commit -s`); contributions are licensed under MPL-2.0. Please also read our **[Code of Conduct](CODE_OF_CONDUCT.md)**.

## Security

Found a vulnerability? **Do not open a public issue.** See **[SECURITY.md](SECURITY.md)** for private disclosure (email `raj@sustentabilitas.com` or a GitHub private advisory).

## License

Licensed under the **Mozilla Public License 2.0** (MPL-2.0). See **[LICENSE](LICENSE)**.

> **Commercial use is welcome.** We chose MPL-2.0 to encourage companies to build on Synapse
> while keeping improvements flowing back to everyone. You can use Synapse in commercial and
> closed-source products, including hosted services, and your own code stays yours: the
> licence covers Synapse's files, not the code you combine them with. If you distribute a
> modified version of a Synapse file, its source must stay available under MPL-2.0, and we
> ask that you contribute those changes back with a pull request so every user benefits.
