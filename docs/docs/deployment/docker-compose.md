---
sidebar_position: 2
title: Docker Compose
description: An example Docker Compose stack that runs the Synapse gateway with a Postgres cost ledger and a Prometheus server scraping its metrics.
---

Use this stack to run Synapse the way you would in production, on one machine: the gateway
writes its cost ledger to Postgres, and Prometheus scrapes its metrics. It is a starting
point for your own deployment, and a way to explore the ledger and metrics without a cloud
account for anything but the model providers.

:::note
This is an example, not the repository's `docker-compose.e2e.yml`. That file runs Synapse's
live Vertex AI integration tests and is not meant for deployments.
:::

## Files

Create a directory with this layout:

```text
synapse-compose/
├── compose.yaml
├── prometheus.yml
├── sa.json          # Google Cloud service-account key
└── config/
    ├── routes.toml
    ├── pricing.toml
    └── guardrails.toml
```

Copy `config/routes.toml`, `config/pricing.toml` and `config/guardrails.toml` from the
[quickstart](../get-started/quickstart.md#create-the-configuration). They define the
`gemini-flash` and `chat` routes, served by Vertex AI with a fallback to OpenAI, and the
`default` guardrails policy that scans them. The gateway does not start without
`guardrails.toml`.

Save the stack as `compose.yaml`, and replace `my-gcp-project` with your Google Cloud
project:

```yaml title="compose.yaml"
services:
  gateway:
    image: sustentabilitas/synapse-gateway:2.0.0
    platform: linux/amd64
    init: true
    ports:
      - "8080:8080"
      - "9090:9090"
    environment:
      VERTEX_PROJECT_ID: my-gcp-project
      GOOGLE_APPLICATION_CREDENTIALS: /secrets/sa.json
      OPENAI_API_KEY: ${OPENAI_API_KEY:?set OPENAI_API_KEY}
      SYNAPSE_LEDGER_BACKENDS: postgres
      SYNAPSE_LEDGER_POSTGRES_DSN: postgres://synapse:synapse@postgres:5432/synapse
      SYNAPSE_DEFAULT_TENANT: compose-example
      RUST_LOG: info
      NO_COLOR: "1"
    volumes:
      - ./config:/app/config:ro
      - ./sa.json:/secrets/sa.json:ro
    depends_on:
      postgres:
        condition: service_healthy

  postgres:
    image: postgres:17
    environment:
      POSTGRES_USER: synapse
      POSTGRES_PASSWORD: synapse
      POSTGRES_DB: synapse
    volumes:
      - ledger:/var/lib/postgresql/data
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U synapse -d synapse"]
      interval: 2s
      timeout: 5s
      retries: 15

  prometheus:
    image: prom/prometheus:v3.5.0
    ports:
      - "9091:9090"
    volumes:
      - ./prometheus.yml:/etc/prometheus/prometheus.yml:ro
    depends_on:
      - gateway

volumes:
  ledger:
```

Save the Prometheus configuration as `prometheus.yml`:

```yaml title="prometheus.yml"
global:
  scrape_interval: 15s

scrape_configs:
  - job_name: synapse-gateway
    static_configs:
      - targets: ["gateway:9090"]
```

What the stack sets up:

- **`gateway`** uses the Postgres ledger only (`SYNAPSE_LEDGER_BACKENDS: postgres`) and
  creates its `usage_events` table on first start. It waits for Postgres to report healthy,
  because a ledger sink that cannot connect at startup is skipped for the life of the
  process. `platform: linux/amd64` lets the image run on Apple silicon; `init: true` lets it
  stop promptly. `NO_COLOR` keeps colour codes out of the logs. It publishes the API
  (`8080`) and metrics (`9090`) ports; the A2A admin endpoints stay on the container's
  loopback, `127.0.0.1:8081` (see [Docker](docker.md#ports)).
- **`postgres`** keeps the ledger in the `ledger` volume, so it survives
  `docker compose down`. Change the password before you use the stack for anything real.
- **`prometheus`** scrapes the gateway's metrics port every 15 seconds under the job name
  `synapse-gateway`. Its UI is on host port `9091`, because the gateway's metrics already use
  `9090`.

On Linux, UID 1001 inside the gateway container must be able to read `sa.json` and the files
in `config/`: `chmod a+r sa.json config/*` is enough on a development machine.

## Start the stack

```bash
export OPENAI_API_KEY=sk-...
docker compose up -d
docker compose logs gateway
```

The gateway's log should include `ledger sink connected backend="postgres"` and
`synapse-gateway listening`. Then send a request:

```bash
curl -s http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "x-synapse-tenant: my-team" \
  -d '{
    "model": "chat",
    "messages": [{"role": "user", "content": "Say hello in Portuguese."}]
  }'
```

## Query the ledger

Each completed request is a row in `usage_events`:

```bash
docker compose exec postgres psql -U synapse -d synapse -c \
  "SELECT ts, tenant, route, provider, model, input_tokens, output_tokens, cost_usd
     FROM usage_events ORDER BY ts DESC LIMIT 10;"
```

Spend per tenant per day, in Postgres syntax:

```sql
SELECT tenant, date_trunc('day', ts) AS day, SUM(cost_usd) AS usd
FROM usage_events
GROUP BY tenant, day
ORDER BY day, usd DESC;
```

The [cost ledger guide](../guides/cost-ledger.md) describes every column and what gets a row.

## Explore the metrics

Open Prometheus at `http://localhost:9091`. **Status → Targets** should show the
`synapse-gateway` job as up. Try the request rate per route and lane:

```text
sum by (route, lane) (rate(synapse_requests_total[5m]))
```

[Queries](../operating/metrics.md#queries) has more, for latency, tokens, fallback and Jev
routing.

You can also read the raw metrics from the gateway with `curl -s http://localhost:9090/metrics`.

To chart them, add Grafana to the stack with the example dashboard: see
[Provision it with Docker Compose](../operating/grafana-dashboard.md#provision-it-with-docker-compose).

## Stop the stack

```bash
docker compose down        # keep the ledger volume
docker compose down -v     # delete the ledger too
```

## Next steps

- Add a Pub/Sub or SNS sink next to Postgres, with `SYNAPSE_LEDGER_BACKENDS: postgres,pubsub`
  and the variables in [Ledger](../configuration/environment-variables.md#ledger).
- Tune the scanners in `config/guardrails.toml` and give routes their own policies; see
  [Guardrails policy](../configuration/guardrails-policy.md).
- Work through the [production checklist](production-checklist.md) before exposing the
  gateway to real traffic.
