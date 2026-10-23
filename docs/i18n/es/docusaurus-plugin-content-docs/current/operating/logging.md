---
sidebar_position: 2
title: Logs
description: Qué registra en los logs el gateway de Synapse, en qué formato y cómo filtrarlo con RUST_LOG.
---

Lee los logs del gateway para confirmar qué cargó al arrancar, para ver por qué falló un destino
del registro de costes o una decisión de enrutamiento de Jev, y para detectar problemas de
configuración. Los logs son para operadores: registran los eventos propios del gateway, no una
línea por petición, y nunca los prompts ni la salida de los modelos.

## Formato {#format}

El gateway escribe una línea por evento en la salida estándar, con el formato de texto plano
del crate [`tracing`](https://docs.rs/tracing): una marca de tiempo UTC, el nivel, el target (el
módulo de Rust que escribió el log), el mensaje y los campos que haya.

```text
2026-09-28T20:12:50.594884Z  INFO synapse_gateway: synapse-gateway metrics listening addr=0.0.0.0:9090 otlp=false
2026-09-28T20:12:50.771039Z  INFO synapse::ledger::connect: ledger sink connected backend="postgres"
2026-09-28T20:12:50.779484Z  INFO synapse_gateway: synapse-gateway listening addr=0.0.0.0:8080
```

Las líneas incluyen códigos de color ANSI salvo que la variable de entorno `NO_COLOR` tenga un
valor no vacío. Define `NO_COLOR=1` en los contenedores para que tu recolector de logs reciba
texto plano. No hay formato de salida JSON.

## Filtrar con `RUST_LOG` {#filter-with-rust_log}

`RUST_LOG` define qué eventos se escriben, con la
[sintaxis de filtros](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html)
de `tracing`. El valor por defecto es `info`. Un valor no válido también equivale a `info`.

| Target | Qué se registra bajo él |
|---|---|
| `synapse_gateway` | El binario: mensajes de arranque y de los listeners. |
| `synapse` | La biblioteca del gateway, como `synapse::ledger::connect`, `synapse::resilience` y `synapse::routing::table`. |
| `synapse::guard` | Resultados de guardrails distintos de `pass` (`block`, `observe`, `masked`, `flag`) y fallos del clasificador Jev. |
| `synapse::routing` | Decisiones del router Jev y conmutaciones (fallbacks) entre tramos de una ruta. |
| `synapse_a2a` | La carga inicial del registro A2A. |
| Otros | Dependencias, como `sqlx`. |

Ejemplos:

```bash
RUST_LOG=info                     # el valor por defecto
RUST_LOG=warn,synapse=info        # solo advertencias de las dependencias
RUST_LOG=info,sqlx=warn           # oculta los avisos de Postgres al arrancar
RUST_LOG=info,synapse::routing=warn   # omite los eventos "route planned" por petición
RUST_LOG=info,synapse::guard=warn     # solo eventos block y observe (dry-run) de guardrails
```

Con el registro de costes en Postgres, `sqlx` escribe un aviso `info` por cada columna que el
gateway comprueba al crear la tabla `usage_events`, como
`column "user_id" of relation "usage_events" already exists, skipping`. Son inofensivos;
`sqlx=warn` los oculta.

## Qué se registra {#what-is-logged}

### Al arrancar {#at-startup}

| Nivel | Mensaje | Significado |
|---|---|---|
| `INFO` | `synapse-gateway metrics listening` | El listener de métricas está activo; `otlp` indica si el envío por OTLP está activado. |
| `INFO` | `ledger sink connected` | Un destino del registro se conectó; `backend` indica cuál. |
| `ERROR` | `ledger sink connect failed; skipping` | Un destino del registro no pudo conectarse y no registra nada hasta que se reinicie el gateway; `error` indica el motivo. |
| `WARN` | `no ledger sinks available; usage accounting disabled` | No se conectó ningún destino. El gateway sirve peticiones sin registrar el uso. |
| `WARN` | `retrying pubsub ledger connect` | El destino de Pub/Sub está reintentando la conexión. |
| `WARN` | `dropping route legs: ...` | La validación de proveedores `lenient` eliminó los tramos de un proveedor porque falta su credencial. |
| `WARN` | `lenient provider validation pruned the route table` | Resumen de lo que eliminó la validación `lenient`, con el número de alias antes y después. |
| `WARN` | `jev route downgraded to static ...` | Una ruta `jev` perdió su router Jev con la validación `lenient`. |
| `WARN` | `jev route default_tier pruned; re-picked` | El `default_tier` de una ruta `jev` perdió todos sus tramos y otro nivel ocupó su lugar. |
| `INFO` | `a2a seed file absent; starting with empty A2A registry` | No se encontró ningún `a2a.toml`. |
| `INFO` / `WARN` | `seeded a2a agent`, `skipping a2a seed agent: card fetch failed after retries` | Resultados de la carga inicial de A2A, uno por agente. |
| `INFO` | `synapse-gateway listening` | El listener de la API está activo y el gateway está listo. |

Si el gateway no puede arrancar, por ejemplo porque `routes.toml` falta o no es válido, o
porque una credencial de proveedor no está definida con la validación estricta, escribe
`Error:` y la causa en la salida de error estándar y termina con un estado distinto de cero.

### Mientras sirve peticiones {#while-serving}

| Nivel | Target | Mensaje | Significado |
|---|---|---|---|
| `WARN` | `synapse::guard` | `guardrail match` | Un escaneo de guardrails produjo `outcome = "block"` u `outcome = "observe"` (dry-run). Campos: `policy`, `lane`, `direction`, `outcome`, `scanners`. Consulta [Política de guardrails](../configuration/guardrails-policy.md#logs-and-ledger-rows). |
| `INFO` | `synapse::guard` | `guardrail match` | Un escaneo de guardrails produjo `outcome = "masked"` u `outcome = "flag"` (`action = "log"` / severidad `warn`). |
| `WARN` | `synapse::guard` | `jev guard classifier failed after trying all typesafe legs` | El clasificador Jev de una política no dio respuesta válida; `route`, `model`, `error.kind` y `http.status` indican el motivo. |
| `INFO` | `synapse::routing` | `route planned` | Uno por petición en una ruta `jev`, con el modo, el nivel decidido, el resultado, el esfuerzo y las puntuaciones de Jev. Consulta [Router Jev](../guides/jev-router.md#metrics-and-logs). |
| `WARN` | `synapse::routing` | `jev decision timed out; routing to default_tier` | Jev no respondió dentro de `timeout_ms`. |
| `WARN` | `synapse::routing` | `jev decision failed; routing to default_tier` | Jev falló; `error.kind` y `http.status` indican cómo. El cuerpo de la respuesta nunca se registra. |
| `WARN` | `synapse::routing` | `leg failed; falling back to next leg` | Un tramo de la ruta falló y la cadena pasó al siguiente tramo, con `route`, `provider`/`model` fallido, siguiente `provider`/`model` y la causa. Consulta [Cadenas de fallback](../guides/fallback-chains.md). |
| `WARN` | `synapse::resilience` | `retrying outbound call` | Un fallo transitorio (`5xx`, `429`, `408`, timeout/conexión) se está reintentando en el mismo tramo con backoff exponencial (`label`, `attempt`, `next_delay_ms`, `error`). |
| `WARN` / `INFO` | `synapse::resilience` | `circuit breaker opened`, `circuit breaker half-open`, `circuit breaker closed` | El circuit breaker de un proveedor cambió de estado (`name`, `consecutive_failures`). |
| `WARN` | `synapse::ledger` | `ledger write failed` | El único destino del registro rechazó una fila, que se pierde. |
| `WARN` | `synapse::ledger` | `ledger sink write failed` | Uno de varios destinos rechazó una fila; `backend` indica cuál. Los demás destinos sí recibieron la fila. |
| `WARN` | `synapse::ledger` | `ledger background writer stopped` | El escritor del registro terminó; no se registran más filas. |
| `ERROR` | `synapse_gateway` | `metrics server stopped` | El listener de métricas falló. |

Las líneas de fallo del registro llevan el `tenant` de la fila, y `ledger write failed` también
su `request_id`, para que puedas encontrar el uso afectado.

### Qué no se registra {#not-logged}

- **Peticiones.** No hay log de accesos. Usa los logs de accesos del proxy o del balanceador de
  carga que tengas delante del gateway.
- **Contenido.** Los prompts, los mensajes, los cuerpos de error de proveedores y la salida de los modelos nunca se registran.

## Trazas {#tracing}

El gateway solo emite logs. No crea spans de traza de OpenTelemetry ni exporta trazas, así que
una petición que pasa por Synapse no aparece en una traza distribuida, y `RUST_LOG` solo
controla los logs. Para telemetría a nivel de petición, usa las [métricas](metrics.md) y el
[registro de costes](../guides/cost-ledger.md), cuyo `request_id` puedes fijar por petición con
el encabezado `x-synapse-message`; consulta
[Correlacionar peticiones](../guides/tenant-attribution.md#correlate-requests). Las trazas
figuran en [Limitaciones y hoja de ruta](../reference/limitations-roadmap.md#observability).
