# synapse-a2a

[![crates.io](https://img.shields.io/crates/v/synapse-a2a.svg)](https://crates.io/crates/synapse-a2a)
[![License: MPL-2.0](https://img.shields.io/badge/License-MPL--2.0-brightgreen.svg)](https://github.com/sustentabilitas/synapse-gateway/blob/main/LICENSE)
[![CI](https://github.com/sustentabilitas/synapse-gateway/actions/workflows/ci.yml/badge.svg)](https://github.com/sustentabilitas/synapse-gateway/actions/workflows/ci.yml)

synapse-a2a is an in-memory registry of agent-to-agent ([A2A](https://a2a-protocol.org/))
agents. Services that host agents register them over HTTP or list them in a seed file;
clients discover them through a catalogue, fetch their agent cards, and resolve an agent id
to the URL where it serves A2A. The registry stores what it is given and hands it back: it
doesn't proxy A2A traffic.

The [synapse-gateway](https://crates.io/crates/synapse-gateway) binary serves the registry,
seeded from `a2a.toml` when that file exists. The public routes are on its API port (default
`:8080`) and the admin routes on `SYNAPSE_ADMIN_ADDR` (default `127.0.0.1:8081`; `shared`
serves them on the API port). This crate lets you serve the same endpoints from your own axum
application.

**Full documentation:** https://synapse-gateway.readthedocs.io/en/latest/docs/synapse-family/a2a/overview/

## Install

```bash
cargo add synapse-a2a
```

## Example

Share one registry between the admin and public routers, so registrations are visible to
discovery:

```rust
use std::sync::Arc;

use synapse_a2a::{a2a_admin_router, a2a_public_router, seed_from_path, A2aRegistry};

async fn app() -> anyhow::Result<axum::Router> {
    let registry = Arc::new(A2aRegistry::new());

    let seed = "config/a2a.toml";
    if std::path::Path::new(seed).exists() {
        seed_from_path(&registry, seed).await?;
    }

    Ok(axum::Router::new()
        .merge(a2a_admin_router(registry.clone()))
        .merge(a2a_public_router(registry)))
}
```

The routers serve:

| Method | Path | Purpose |
|---|---|---|
| `POST` | `/internal/a2a/agents` | Register an agent (first writer wins). |
| `DELETE` | `/internal/a2a/agents/{id}` | Remove an agent. |
| `GET` | `/.well-known/a2a-agent-catalog.json` | List the agents. |
| `GET` | `/a2a/agents/{id}/.well-known/agent-card.json` | One agent's card. |
| `GET` | `/a2a/agents/{id}/resolve` | One agent's endpoint and card. |

The `/internal/` endpoints have no authentication: serve them on a private listener, or block
them at your ingress.

## Learn more

- [Static seed](https://synapse-gateway.readthedocs.io/en/latest/docs/synapse-family/a2a/static-seed/)
- [Admin API](https://synapse-gateway.readthedocs.io/en/latest/docs/synapse-family/a2a/admin-api/)
- [Discovery](https://synapse-gateway.readthedocs.io/en/latest/docs/synapse-family/a2a/discovery/)
- [Crate API](https://synapse-gateway.readthedocs.io/en/latest/docs/synapse-family/a2a/crate-api/) and [docs.rs](https://docs.rs/synapse-a2a)

## License

Licensed under the **Mozilla Public License 2.0** (MPL-2.0), which welcomes commercial use and asks that changes to Synapse's own files are shared back. See **[LICENSE](https://github.com/sustentabilitas/synapse-gateway/blob/main/LICENSE)** and the [licence note](https://github.com/sustentabilitas/synapse-gateway#license).
