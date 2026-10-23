---
sidebar_position: 8
title: Registro de costes
description: Qué anota el registro de costes de Synapse por cada petición, dónde lo escribe, qué fiabilidad tienen las cifras y cómo consultar el gasto por tenant.
---

Usa el registro de costes para responder a "quién gastó qué, en qué modelo y para qué tipo de
trabajo". Synapse escribe una fila por cada petición completada, con su tenant, su ruta, el
modelo que la sirvió, el número de tokens y el coste, en una base de datos que es tuya, y
puede publicar los mismos eventos en un bus de mensajes para pipelines de facturación o
analítica. Está pensado para informes de uso e imputación interna de costes (showback), no
como una factura exacta: lee [Precisión](#accuracy) antes de facturar a clientes a partir de
él.

## Qué genera una fila {#what-gets-a-row}

| Evento | `lane` | `provider` y `model` |
|---|---|---|
| Un chat completion servido en el carril estándar | `standard` | El tramo que lo sirvió. |
| Un chat completion servido en el carril Vertex nativo | `native` | El tramo `vertex` que lo sirvió. |
| Una respuesta del carril Jev, o la respuesta de Jev de una extracción híbrida | `jev` | `typesafe` y la build de Jev. |
| Una decisión del router Jev | `jev` | `typesafe` y `jev_router.model`. |
| Cada extracción híbrida | `standard` o `native` | El tramo que la sirvió. |
| Una petición de embeddings | `embedding` | El tramo que la sirvió; `output_tokens` es 0. |
| Una llamada passthrough de Gemini `generateContent` o `streamGenerateContent` | `passthrough` | `vertex` y el modelo del tramo. |
| Una llamada passthrough de Jev | `passthrough` | `typesafe` y el modelo llamado. |
| Un bloqueo de escáner de guardrail de entrada (`op = "guard_input"`) | `guard` | `guard` y el nombre del escáner activado (por ejemplo `prompt_injection`); 0 tokens y coste `$0`. |
| Una comprobación del clasificador Jev de guardrails (`op = "guard_classifier"`) | `guard` | `typesafe` y `classifier.model`. |

Las peticiones de chat y de embeddings fallidas no escriben ninguna fila: una petición sin
streaming que falla, una petición con streaming que falla antes de su primer fragmento o una
petición rechazada con `400`. Los bloqueos de guardrails de entrada y las comprobaciones del
clasificador Jev escriben una fila con `lane = "guard"` (`status = "blocked"` cuando la
petición se bloquea, o `status = "ok"` cuando el clasificador permite la petición o se ejecuta
en modo `observe` / `log`). Una respuesta con streaming escribe su fila cuando termina el
stream, tanto si se completa como si falla o el cliente se desconecta; un stream que falla a
mitad de camino se registra con `status` `error`. Las llamadas passthrough escriben una fila
incluso cuando fallan. Otras acciones passthrough de Gemini, como `countTokens`, se reenvían
sin fila.

## Formato de las filas {#row-format}

El gateway crea la tabla `usage_events` al arrancar, en SQLite y en Postgres:

| Columna | Descripción |
|---|---|
| `id` | Id de la fila. |
| `ts` | Cuándo terminó la petición, en UTC. SQLite lo guarda como texto RFC 3339. |
| `tenant`, `workspace`, `user_id`, `thread_id`, `message_id` | Atribución a partir de los encabezados de la petición; consulta [Atribución por tenant](tenant-attribution.md). |
| `route` | La ruta o el alias de embeddings al que llamó el cliente. |
| `provider`, `model` | El tramo que sirvió la petición, o `guard` / `typesafe` para las filas de guardrails. |
| `lane` | `standard`, `native`, `jev`, `embedding`, `passthrough` o `guard`. |
| `input_tokens`, `output_tokens` | Número de tokens informado por el proveedor. |
| `cost_usd` | Coste calculado a partir de `pricing.toml`. Consulta [Precios](../configuration/pricing.md#how-cost-is-computed). |
| `request_id` | El id de la petición, compartido por todas las filas que escribe una misma petición. |
| `status` | `ok`, `blocked` (para comprobaciones de guardrails bloqueadas), o `error` para streams y llamadas passthrough fallidos. |
| `user_task_type`, `ai_task_type` | Clasificación de la tarea; consulta [Tipos de tarea de IA](tenant-attribution.md#ai-task-types). |

## Destinos {#sinks}

El registro escribe en uno o más destinos, seleccionados con `SYNAPSE_LEDGER_BACKENDS`:

- **`sqlite`**, el valor por defecto, escribe en un archivo local. Adecuado para una sola
  instancia y para probar Synapse.
- **`postgres`** escribe en una base de datos compartida. Úsalo cuando varias instancias del
  gateway deban compartir un mismo registro.
- **`pubsub`** y **`sns`** publican cada fila como un evento JSON en Google Cloud Pub/Sub o en
  AWS SNS, para pipelines de facturación, analítica o almacén de datos.

Cada fila va a todos los destinos configurados, de forma concurrente; el fallo de un destino
no afecta a los demás. Los destinos Postgres, Pub/Sub y SNS necesitan sus features de Cargo,
que la imagen de Docker incluye. [Registro](../configuration/environment-variables.md#ledger)
en la referencia de variables de entorno enumera los ajustes de conexión de cada destino.

Un destino que no puede conectarse al arrancar se anota en el log y se omite. Si no se conecta
ninguno, el gateway arranca igualmente y sirve peticiones sin registrar nada, así que revisa
los logs de arranque después de configurar un destino.

## Entrega {#delivery}

Las escrituras en el registro nunca ralentizan una petición. Synapse pone cada fila en una
cola en memoria de 10,000 filas y una tarea en segundo plano las escribe. Ese diseño tiene
consecuencias:

- Si la cola está llena, las filas nuevas se descartan y se cuentan en
  `synapse_ledger_dropped_total`.
- Una escritura fallida se anota en el log y se cuenta en `synapse_ledger_errors_total`, y no
  se reintenta. Con varios destinos, la etiqueta `backend` nombra el destino que falla; con uno
  solo, es `writer`.
- Las filas que siguen en la cola cuando el proceso se detiene se pierden.

Configura alertas para cuando cualquiera de los dos contadores sea mayor que cero. Para el uso
que no puedes perder, publica en Pub/Sub o SNS además de en una base de datos, de modo que la
caída de un único destino no deje un hueco.

## Eventos publicados {#published-events}

Los destinos Pub/Sub y SNS publican un evento JSON por fila, en camelCase, con el tenant como
`namespace`:

```json
{
  "namespace": "my-team",
  "workspace": "onboarding",
  "requestId": "0f8e5c1e-2d1b-4c7a-9a55-3f0c6f7d9b21",
  "timestamp": "2026-09-28T10:00:00Z",
  "type": "usage",
  "route": "chat",
  "provider": "vertex",
  "model": "gemini-3.5-flash-lite",
  "lane": "standard",
  "inputTokens": 128,
  "outputTokens": 256,
  "costUsd": 0.0006784,
  "status": "ok",
  "op": "chat",
  "aiTaskType": "conversation"
}
```

`workspace`, `user`, `threadId`, `messageId` y `userTaskType` solo aparecen cuando tienen
valor. `op` indica qué produjo el evento: `chat` para un chat completion o una llamada
passthrough de Gemini, `embedding`, `route_decision` para una decisión del router Jev,
`guard_input` para un bloqueo de escáner de guardrail de entrada, `guard_classifier` para una
comprobación del clasificador Jev de guardrails, o `systemone` para una llamada passthrough de
Jev. Las tablas de la base de datos no tienen columna `op`.

Cada mensaje lleva también atributos para filtros de suscripción: `EventType`
(`Ledger.LLMTokensConsumed`), `namespace`, `requestId`, `type`, `provider` y `status`. Los
mensajes de Pub/Sub usan `requestId` como clave de ordenación.

## Precisión {#accuracy}

El registro solo es tan preciso como los recuentos de tokens y los precios en que se basa.
Carencias conocidas:

- **La entrada en caché en el carril Vertex nativo se valora a precio completo.**
  `input_tokens` es el `promptTokenCount` de Vertex AI, que incluye los tokens leídos de una
  caché de contexto, y Synapse los valora todos al precio `input`. Vertex AI factura los tokens
  en caché con descuento y cobra aparte el almacenamiento de la caché, así que las peticiones
  con caché aparecen sobrestimadas.
- **Los tokens de razonamiento en el carril Vertex nativo no se cuentan.** `output_tokens` es
  el `candidatesTokenCount` de Vertex AI, que excluye los tokens de razonamiento, y Vertex AI
  factura el razonamiento como salida. Las peticiones que razonan aparecen subestimadas. En
  el carril estándar, los tokens de razonamiento de Gemini sí se incluyen en `output_tokens`.
- **Los intentos fallidos no se registran.** No aparecen los tokens que un proveedor consumió
  en un tramo que falló antes de que otro tramo sirviera la petición.
- **Los streams abandonados registran cero tokens.** Los proveedores informan del uso al final
  del stream, así que una petición con streaming de la que el cliente se desconecta pronto se
  registra con 0 tokens de entrada y 0 de salida.
- **Los modelos de chat sin precio cuestan 0.** Un modelo de chat que no está en
  `pricing.toml` se registra con `cost_usd` 0. Los modelos de embeddings recurren en cambio a
  un precio por defecto.
- **Filas descartadas.** Consulta [Entrega](#delivery).

Las filas guardan los recuentos de tokens en bruto junto al coste, así que puedes recalcular
el coste con tus propias tarifas.

## Consultar el registro {#query-the-ledger}

Estas consultas usan la sintaxis de SQLite. Gasto por tenant y por día:

```sql
SELECT tenant, substr(ts, 1, 10) AS day, SUM(cost_usd) AS usd,
       SUM(input_tokens) AS input_tokens, SUM(output_tokens) AS output_tokens
FROM usage_events
GROUP BY tenant, day
ORDER BY day, usd DESC;
```

Coste por tipo de trabajo y modelo:

```sql
SELECT ai_task_type, provider, model, COUNT(*) AS requests, SUM(cost_usd) AS usd
FROM usage_events
WHERE status = 'ok'
GROUP BY ai_task_type, provider, model
ORDER BY usd DESC;
```

El coste de una petición enrutada por Jev, decisión incluida:

```sql
SELECT request_id, SUM(cost_usd) AS usd, GROUP_CONCAT(model) AS models
FROM usage_events
WHERE route = 'auto'
GROUP BY request_id;
```
