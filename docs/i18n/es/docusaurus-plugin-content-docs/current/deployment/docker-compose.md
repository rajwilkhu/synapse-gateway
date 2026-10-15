---
sidebar_position: 2
title: Docker Compose
description: Un stack de ejemplo de Docker Compose que ejecuta el gateway Synapse con un registro de costes en Postgres y un servidor Prometheus que recoge sus métricas.
---

Usa este stack para ejecutar Synapse como lo harías en producción, en una sola máquina: el
gateway escribe su registro de costes en Postgres y Prometheus recoge sus métricas. Es un punto
de partida para tu propio despliegue, y una forma de explorar el registro de costes y las
métricas sin una cuenta en la nube para nada más que los proveedores de modelos.

:::note
Esto es un ejemplo, no el `docker-compose.e2e.yml` del repositorio. Ese archivo ejecuta las
pruebas de integración en vivo de Synapse con Vertex AI y no está pensado para despliegues.
:::

## Archivos {#files}

Crea un directorio con esta estructura:

```text
synapse-compose/
├── compose.yaml
├── prometheus.yml
├── sa.json          # clave de cuenta de servicio de Google Cloud
└── config/
    ├── routes.toml
    ├── pricing.toml
    └── guardrails.toml
```

Copia `config/routes.toml`, `config/pricing.toml` y `config/guardrails.toml` del
[inicio rápido](../get-started/quickstart.md#create-the-configuration). Definen las rutas
`gemini-flash` y `chat`, servidas por Vertex AI con fallback a OpenAI, y la política de
guardrails `default` que las escanea. El gateway no arranca sin `guardrails.toml`.

Guarda el stack como `compose.yaml` y sustituye `my-gcp-project` por tu proyecto de Google
Cloud:

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

Guarda la configuración de Prometheus como `prometheus.yml`:

```yaml title="prometheus.yml"
global:
  scrape_interval: 15s

scrape_configs:
  - job_name: synapse-gateway
    static_configs:
      - targets: ["gateway:9090"]
```

Qué configura el stack:

- **`gateway`** usa solo el registro de costes en Postgres (`SYNAPSE_LEDGER_BACKENDS: postgres`)
  y crea su tabla `usage_events` en el primer arranque. Espera a que Postgres informe de que
  está sano, porque un destino del registro de costes que no puede conectar al arrancar se
  omite durante toda la vida del proceso. `platform: linux/amd64` permite ejecutar la imagen en
  Apple silicon; `init: true` le permite pararse enseguida. `NO_COLOR` mantiene los códigos de
  color fuera de los logs. Publica los puertos de la API (`8080`) y de métricas (`9090`); los
  endpoints de administración de A2A se quedan en el loopback del contenedor,
  `127.0.0.1:8081` (consulta [Docker](docker.md#ports)).
- **`postgres`** guarda el registro de costes en el volumen `ledger`, así que sobrevive a
  `docker compose down`. Cambia la contraseña antes de usar el stack para algo real.
- **`prometheus`** recoge las métricas del puerto de métricas del gateway cada 15 segundos con
  el nombre de job `synapse-gateway`. Su interfaz está en el puerto `9091` del host, porque las
  métricas del gateway ya usan el `9090`.

En Linux, el UID 1001 dentro del contenedor del gateway debe poder leer `sa.json` y los
archivos de `config/`: en una máquina de desarrollo basta con `chmod a+r sa.json config/*`.

## Arrancar el stack {#start-the-stack}

```bash
export OPENAI_API_KEY=sk-...
docker compose up -d
docker compose logs gateway
```

El log del gateway debería incluir `ledger sink connected backend="postgres"` y
`synapse-gateway listening`. Después envía una petición:

```bash
curl -s http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "x-synapse-tenant: my-team" \
  -d '{
    "model": "chat",
    "messages": [{"role": "user", "content": "Say hello in Portuguese."}]
  }'
```

## Consultar el registro de costes {#query-the-ledger}

Cada petición completada es una fila en `usage_events`:

```bash
docker compose exec postgres psql -U synapse -d synapse -c \
  "SELECT ts, tenant, route, provider, model, input_tokens, output_tokens, cost_usd
     FROM usage_events ORDER BY ts DESC LIMIT 10;"
```

Gasto por tenant y por día, en sintaxis de Postgres:

```sql
SELECT tenant, date_trunc('day', ts) AS day, SUM(cost_usd) AS usd
FROM usage_events
GROUP BY tenant, day
ORDER BY day, usd DESC;
```

La [guía del registro de costes](../guides/cost-ledger.md) describe cada columna y qué genera
una fila.

## Explorar las métricas {#explore-the-metrics}

Abre Prometheus en `http://localhost:9091`. **Status → Targets** debería mostrar el job
`synapse-gateway` como activo. Prueba la tasa de peticiones por ruta y carril:

```text
sum by (route, lane) (rate(synapse_requests_total[5m]))
```

[Consultas](../operating/metrics.md#queries) tiene más, para latencia, tokens, fallback y
enrutamiento de Jev.

También puedes leer las métricas en bruto del gateway con
`curl -s http://localhost:9090/metrics`.

Para representarlas en gráficos, añade Grafana al stack con el panel de ejemplo: consulta
[Aprovisionarlo con Docker Compose](../operating/grafana-dashboard.md#provision-it-with-docker-compose).

## Parar el stack {#stop-the-stack}

```bash
docker compose down        # conserva el volumen del registro de costes
docker compose down -v     # borra también el registro de costes
```

## Siguientes pasos {#next-steps}

- Añade un destino Pub/Sub o SNS junto a Postgres, con
  `SYNAPSE_LEDGER_BACKENDS: postgres,pubsub` y las variables de
  [Registro de costes](../configuration/environment-variables.md#ledger).
- Ajusta los escáneres de `config/guardrails.toml` y asigna políticas propias a las rutas;
  consulta [Política de guardrails](../configuration/guardrails-policy.md).
- Recorre la [lista de comprobación para producción](production-checklist.md) antes de exponer
  el gateway a tráfico real.
