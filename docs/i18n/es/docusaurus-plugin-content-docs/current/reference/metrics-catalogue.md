---
sidebar_position: 3
title: Catálogo de métricas
description: Todas las métricas que registran el gateway, synapse-proxy y synapse-mcp, con su tipo, unidad y etiquetas.
---

Esta página lista todos los instrumentos de OpenTelemetry del workspace, con el nombre que tienen
en Prometheus. Para saber cómo exporta métricas el gateway, qué cuentan las métricas de
peticiones y con qué consultas y alertas empezar, consulta [Métricas](../operating/metrics.md).

Ninguno de los instrumentos declara una unidad en el código. Las duraciones se registran en
segundos, como indica el sufijo `_seconds`; los contadores cuentan eventos o tokens.

## Gateway {#gateway}

El gateway las sirve en su puerto de métricas, `SYNAPSE_METRICS_ADDR` (por defecto
`0.0.0.0:9090`), y las envía por OTLP cuando `OTEL_EXPORTER_OTLP_ENDPOINT` está definida. Los
nombres en Prometheus son exactamente los listados. Los histogramas tienen los buckets 0.005,
0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1, 2.5, 5, 10, 30, 60 y 120 segundos; las peticiones más
lentas solo caen en el bucket `+Inf`.

### Peticiones de chat {#chat-requests}

| Métrica | Tipo | Unidad | Etiquetas |
|---|---|---|---|
| `synapse_requests_total` | Counter | peticiones | `route`, `model`, `system`, `lane` |
| `synapse_request_duration_seconds` | Histogram | segundos | `route`, `model`, `system`, `lane` |
| `synapse_input_tokens_total` | Counter | tokens | `route`, `model`, `system`, `lane` |
| `synapse_output_tokens_total` | Counter | tokens | `route`, `model`, `system`, `lane` |

- `route` es el alias de la ruta y `model` es el modelo del tramo que sirvió la petición.
- `system` es la familia de proveedor del tramo que sirvió: `vertexai` para `vertex`, `openai`,
  `dashscope` para `qwen`, y `oai_compat` para `oai_compat` y `typesafe`.
- `lane` es `standard`, `native` o `jev`. Una respuesta de Jev con streaming se etiqueta como
  `standard`.

Solo se cuentan las peticiones que produjeron una respuesta; consulta
[Qué cuentan las métricas de peticiones](../operating/metrics.md#what-the-request-metrics-count).

### Embeddings {#embeddings}

| Métrica | Tipo | Unidad | Etiquetas |
|---|---|---|---|
| `synapse_embeddings_total` | Counter | peticiones | `route`, `model`, `provider` |
| `synapse_embedding_duration_seconds` | Histogram | segundos | `route`, `model`, `provider` |

`route` es el alias de embeddings; `model` y `provider` indican el tramo que la sirvió. Solo se
cuentan las peticiones con éxito, y la duración cubre únicamente el tramo que sirvió. Consulta
[Embeddings](../guides/embeddings.md#metrics).

### Passthrough {#passthrough}

| Métrica | Tipo | Unidad | Etiquetas |
|---|---|---|---|
| `synapse_passthrough_total` | Counter | llamadas | `provider`, `model`, `action`, `status` |
| `synapse_passthrough_fallback_total` | Counter | fallbacks | `from_model`, `to_model` |

- `synapse_passthrough_total` cuenta cada intento de los passthroughs de Gemini y Jev.
  `provider` es `vertex` o `typesafe`; `action` es la acción de Gemini, o `systemone`; `status`
  es `ok` para una respuesta `2xx` y `error` en otro caso.
- `synapse_passthrough_fallback_total` cuenta cada paso al siguiente tramo `vertex` en un
  passthrough de Gemini.

### Jev {#jev}

| Métrica | Tipo | Unidad | Etiquetas |
|---|---|---|---|
| `synapse_routing_decisions_total` | Counter | peticiones | `route`, `tier`, `outcome` |
| `synapse_routing_decision_duration_seconds` | Histogram | segundos | `route` |
| `synapse_jev_extraction_total` | Counter | respuestas | `route`, `degraded` |

- `synapse_routing_decisions_total` cuenta las peticiones a rutas `strategy = "jev"`. `tier` es
  el nivel que seleccionó la decisión; `outcome` es `decided`, `low_confidence`, `timeout`,
  `error` o `static_override`. Consulta [Router Jev](../guides/jev-router.md#metrics-and-logs).
- `synapse_routing_decision_duration_seconds` mide la llamada a Jev, así que las peticiones con
  `"routing_strategy": "static"` no la registran.
- `synapse_jev_extraction_total` cuenta las respuestas de extracción híbrida; `degraded` es
  `true` o `false`. Consulta [Carril Jev](../guides/jev-lane.md#ledger-and-metrics).

### Guardrails {#guardrails}

| Métrica | Tipo | Unidad | Etiquetas |
|---|---|---|---|
| `synapse_guard_scans_total` | Counter | escaneos | `lane`, `policy`, `outcome` |
| `synapse_guard_matches_total` | Counter | coincidencias | `lane`, `policy`, `scanner`, `severity` |
| `synapse_guard_scan_duration_seconds` | Histogram | segundos | `lane`, `policy` |

`lane` es `chat`, `passthrough`, `embeddings` o `jev`, un conjunto de valores distinto del de
la etiqueta `lane` de `synapse_requests_total` (`standard`, `native` o `jev`). `outcome` es
`pass`, `flag`, `block` u `observe`; `severity` es `block`, `warn` o `info`.
Consulta [Política de guardrails](../configuration/guardrails-policy.md#metrics).

### Registro de costes {#cost-ledger}

| Métrica | Tipo | Unidad | Etiquetas |
|---|---|---|---|
| `synapse_ledger_dropped_total` | Counter | filas | ninguna |
| `synapse_ledger_errors_total` | Counter | escrituras fallidas | `backend` |

- `synapse_ledger_dropped_total` cuenta las filas descartadas porque la cola del registro estaba
  llena.
- `synapse_ledger_errors_total` cuenta las filas que un destino no pudo escribir. Con un solo
  destino, `backend` es `writer`. Con varios, es el nombre del destino que falla: `sqlite`,
  `postgres`, `pubsub` o `sns`.

Consulta [Entrega](../guides/cost-ledger.md#delivery).

### Resiliencia {#resilience}

| Métrica | Tipo | Unidad | Etiquetas |
|---|---|---|---|
| `synapse_resilience_calls_total` | Counter | llamadas | `label`, `outcome` |
| `synapse_resilience_call_duration_seconds` | Histogram | segundos | `label`, `outcome` |
| `synapse_resilience_retry_attempts_total` | Counter | reintentos | `label` |
| `synapse_resilience_breaker_transitions_total` | Counter | transiciones | `name`, `transition` |
| `synapse_resilience_breaker_state` | Gauge | estado | `name`: 0 cerrado, 1 abierto, 2 semiabierto |

:::note
Estos instrumentos están definidos, pero el gateway actual nunca los registra: las peticiones de
chat no tienen reintentos ni circuit breakers. No aparecen en la exposición.
:::

## synapse-proxy {#synapse-proxy}

El binario `synapse-proxy` las sirve en su `metrics_addr` (por defecto `0.0.0.0:9090`) en
`GET /metrics`, solo en formato Prometheus. Una aplicación que embebe la biblioteca del proxy
puede añadir un exportador OTLP con `Metrics::with_otlp`.

El proxy usa la configuración por defecto del exportador de Prometheus de OpenTelemetry, así que
su exposición difiere de la del gateway: los contadores reciben un segundo sufijo `_total`, cada
serie lleva `otel_scope_name="synapse-proxy"` y hay una serie `target_info`. Los nombres de abajo
son los que ve Prometheus:

| Métrica | Tipo | Unidad | Etiquetas |
|---|---|---|---|
| `synapse_proxy_requests_total_total` | Counter | peticiones | `route`, `method`, `status`, `outcome` |
| `synapse_proxy_request_duration_seconds` | Histogram | segundos | `route`, `method` |
| `synapse_proxy_upstream_retries_total_total` | Counter | reintentos | `route`, `reason` |
| `synapse_proxy_upstream_errors_total_total` | Counter | errores | `route`, `reason` |
| `synapse_proxy_transform_errors_total_total` | Counter | errores | `route`, `transform` |

- `route` es el `name` de la ruta del proxy, o `none` cuando no coincidió ninguna ruta. `status`
  es el estado HTTP devuelto al cliente.
- `outcome` es `forwarded`, `no_route`, `context_unbound`, `transform_rejected` o
  `upstream_error`. Una petición rechazada por un cuerpo demasiado grande no se cuenta.
- La duración corre hasta que los encabezados de la respuesta están listos, no hasta que termina
  un cuerpo transmitido en streaming.
- `synapse_proxy_upstream_retries_total_total` cuenta los envíos reintentados; `reason` es
  `connect` o `send`.
- `synapse_proxy_upstream_errors_total_total` cuenta los envíos que fallaron tras todos los
  reintentos; `reason` es siempre `send`.
- `synapse_proxy_transform_errors_total_total` cuenta los rechazos de una transformación de
  petición o de respuesta; `transform` es `request` o `response`.

:::warning Buckets del histograma
`synapse_proxy_request_duration_seconds` usa los buckets por defecto de OpenTelemetry, que están
dimensionados para milisegundos (0, 5, 10, 25, 50, 75, 100, 250, 500, 750, 1000, 2500, 5000,
7500 y 10000), mientras que los valores están en segundos. Toda petición de menos de 5 segundos
cae entre los límites `le="0"` y `le="5"`, así que los cuantiles calculados a partir de este
histograma no tienen sentido por debajo de 5 segundos. Usa `_sum / _count` para la latencia
media.
:::

## synapse-mcp {#synapse-mcp}

`synapse-mcp` registra sus instrumentos en un meter que proporciona la aplicación que lo embebe;
el crate no exporta nada por sí mismo.

| Instrumento | Tipo | Unidad | Etiquetas |
|---|---|---|---|
| `broker_mcp_requests_total` | Counter | llamadas a herramientas | `tool`, `upstream`, `outcome` |
| `broker_mcp_request_duration_seconds` | Histogram | segundos | `tool`, `upstream` |
| `broker_identity_injection_failures_total` | Counter | fallos | `reason` |

- `tool` es el nombre de la herramienta MCP y `upstream` es el servidor MCP al que se enrutó la
  llamada. `outcome` es `ok` o `error`.
- `broker_identity_injection_failures_total` cuenta las llamadas rechazadas porque faltaba una
  clave de contexto obligatoria; `reason` es siempre `missing_context_key`.

Estos son nombres de instrumentos. Cuando el meter viene de `Metrics::meter()` de
`synapse-proxy`, se aplica la configuración del exportador del proxy: los contadores aparecen
como `broker_mcp_requests_total_total` y `broker_identity_injection_failures_total_total`, las
series llevan `otel_scope_name="sandbox-broker"` y el histograma tiene los mismos buckets por
defecto dimensionados para milisegundos que el del proxy.
