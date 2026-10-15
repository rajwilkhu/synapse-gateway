---
sidebar_position: 1
title: API HTTP
description: Todos los endpoints que sirve el binario del gateway, con sus formatos de petición y respuesta, los encabezados x-synapse-* y los códigos de error.
---

El binario del gateway sirve tres listeners:

- la **API**, en `SYNAPSE_ADDR` (por defecto `0.0.0.0:8080`), con todos los endpoints de esta
  página salvo las métricas y los endpoints de administración de A2A;
- el endpoint de **métricas**, en `SYNAPSE_METRICS_ADDR` (por defecto `0.0.0.0:9090`);
- los endpoints de **administración de A2A**, `/internal/a2a/*`, en `SYNAPSE_ADMIN_ADDR` (por
  defecto `127.0.0.1:8081`), o en la API con `shared`.

Todos hablan HTTP sin cifrar y ninguno autentica a los clientes; consulta
[Seguridad](../operating/security.md).

## Endpoints {#endpoints}

| Método | Path | Propósito |
|---|---|---|
| `GET` | `/health` | Comprobación de actividad (liveness). |
| `GET` | `/v1/models` | Lista los alias de las rutas de chat. |
| `POST` | `/v1/chat/completions` | Chat completions compatibles con OpenAI, con o sin streaming. |
| `POST` | `/v1/embeddings` | Embeddings compatibles con OpenAI. |
| `POST` | `/v1beta/models/{model}:{action}` | Passthrough nativo de Gemini a Vertex AI. |
| `POST` | `/v1/models/{model}:{action}` | Passthrough nativo de Gemini a Vertex AI. |
| `POST` | `/google/models/{model}:{action}` | Passthrough nativo de Gemini a Vertex AI. |
| `POST` | `/typesafe/v1/systemone` | Passthrough de Jev a TypeSafe System One. |
| `POST` | `/internal/a2a/agents` | Registra un agente A2A (listener de administración, `SYNAPSE_ADMIN_ADDR`). |
| `DELETE` | `/internal/a2a/agents/{id}` | Elimina un agente A2A (listener de administración, `SYNAPSE_ADMIN_ADDR`). |
| `GET` | `/.well-known/a2a-agent-catalog.json` | Lista los agentes A2A registrados. |
| `GET` | `/a2a/agents/{id}/.well-known/agent-card.json` | La agent card A2A de un agente. |
| `GET` | `/a2a/agents/{id}/resolve` | El endpoint y la agent card de un agente. |
| `GET` | `/metrics` y `/` (puerto de métricas) | Métricas de Prometheus. |

Cualquier otro path devuelve `404` con el cuerpo vacío, y un path conocido con el método
equivocado devuelve `405`.

## Encabezados de petición {#request-headers}

Todos los endpoints salvo los de salud, modelos, A2A y métricas leen estos encabezados de
atribución opcionales y los anotan en las filas del [registro de costes](../guides/cost-ledger.md)
de la petición. [Atribución por tenant](../guides/tenant-attribution.md#attribution-headers)
explica cada uno.

| Encabezado | Efecto |
|---|---|
| `x-synapse-tenant` | Tenant. Sin él, `SYNAPSE_DEFAULT_TENANT` (por defecto `unattributed`). |
| `x-synapse-workspace` | Workspace dentro del tenant. |
| `x-synapse-user` | Usuario final. |
| `x-synapse-thread` | Conversación o hilo de agente. |
| `x-synapse-message` | Id de mensaje. En los chat completions y en los passthroughs también se convierte en el id de la petición. |
| `x-synapse-user-task-type` | Tu propia etiqueta para el trabajo, registrada tal como se envía. |
| `x-synapse-ai-task-type` | Sustituye el [tipo de tarea de IA](../guides/tenant-attribution.md#ai-task-types). |

## Encabezados de respuesta {#response-headers}

Las respuestas de los chat completions llevan el informe de enrutamiento.
[Router Jev](../guides/jev-router.md#response-headers) describe cuándo aparece cada encabezado.

| Encabezado | Valores |
|---|---|
| `x-synapse-routing` | `static`, `jev` o `static-override`. Siempre presente en los chat completions. |
| `x-synapse-tier` | El nivel cuyo tramo sirvió la petición. |
| `x-synapse-tier-decided` | El nivel que eligió Jev, cuando sirvió otro nivel. |
| `x-synapse-reasoning-effort` | El esfuerzo del tramo que sirvió, o `client` cuando la petición fijó el suyo. |
| `x-synapse-routing-degraded` | `timeout`, `error`, `low_confidence` o `jev_unavailable`. |
| `x-synapse-guard` | `masked` cuando una política de guardrails reescribió texto de la petición. Aparece en chat completions (con o sin streaming), embeddings, el passthrough de Gemini y el passthrough de Jev. En una chat completion sin streaming, también `masked` cuando el escaneo de salida reescribió la respuesta, y `blocked` cuando la bloqueó. Consulta [Acciones y enmascaramiento](../configuration/guardrails-policy.md#actions-and-masking) y [Escaneo de salida](../configuration/guardrails-policy.md#output-scanning). |

## Errores {#errors}

Los errores del gateway son JSON con la forma de OpenAI:

```json
{
  "error": {
    "type": "model_not_found",
    "message": "unknown model alias 'chat-typo'",
    "code": "model_not_found"
  }
}
```

| Estado | `code` | Cuándo |
|---|---|---|
| `400` | `invalid_request_error` | La petición no es válida para su ruta, por ejemplo un `routing_strategy` desconocido, un bloque `jev` en el tipo de ruta equivocado, un `dimensions` de embeddings que no coincide, o un `4xx` de Vertex nativo que detiene la cadena. También lo devuelve un passthrough cuyo proveedor no está configurado. |
| `400` | `native_feature_unsupported` | La petición usa funcionalidades de Vertex nativo y la ruta no tiene ningún tramo `vertex`. |
| `400` | `content_blocked` | Una política de guardrails bloqueó la petición. `type` es `content_policy_violation` y un array `scanners` nombra los escáneres. Consulta [Respuesta de bloqueo](../configuration/guardrails-policy.md#block-response). |
| `404` | `model_not_found` | El `model` no es un alias de ruta de chat (chat) ni un alias de embeddings (embeddings). |
| `502` | `all_legs_failed` | Fallaron todos los tramos. Un array `failures` lista el `provider`, el `model` y el `message` de cada tramo. Consulta [Cuando fallan todos los tramos](../guides/fallback-chains.md#when-every-leg-fails). |
| `502` | `upstream_error` | Un proveedor falló donde no era posible ningún fallback más, como el último tramo de Vertex nativo o un tramo de embeddings. |
| `503` | `guard_unavailable` | Un clasificador de guardrails no dio respuesta con `on_error = "block"` en modo `block`. Consulta [Clasificador](../configuration/guardrails-policy.md#classifier). |

El `message` es legible para personas y puede cambiar entre versiones; para distinguir errores,
usa `code`. Un cuerpo que no es JSON válido, o que no encaja con el esquema del endpoint, se
rechaza antes de llegar al gateway, con un mensaje de texto plano en lugar de esta forma: `400`
para JSON mal formado, `415` sin un encabezado `Content-Type: application/json`, y `422` para un
campo que falta o tiene un tipo incorrecto. Los cuerpos de más de 2 MB se rechazan con `413`.

Los endpoints passthrough devuelven el estado y el cuerpo del proveedor sin cambios cuando el
proveedor responde. Consulta [Passthrough de Gemini](#gemini-passthrough) y
[Passthrough de Jev](#jev-passthrough).

## `GET /health` {#get-health}

Devuelve `200` con el cuerpo `ok`. No comprueba los proveedores ni el registro de costes; solo
muestra que el proceso está sirviendo HTTP.

## `GET /v1/models` {#get-v1models}

Lista los alias de las rutas de chat de `routes.toml`, ordenados por nombre:

```json
{
  "object": "list",
  "data": [
    { "id": "auto", "object": "model", "owned_by": "synapse" },
    { "id": "chat", "object": "model", "owned_by": "synapse" }
  ]
}
```

Los alias de embeddings no se listan.

## `POST /v1/chat/completions` {#post-v1chatcompletions}

Una petición de chat completion de OpenAI. `model` es un alias de ruta; la ruta decide qué
proveedores y modelos la sirven.

| Campo | Tipo | Descripción |
|---|---|---|
| `model` | string | Obligatorio. Un alias de ruta de `routes.toml`. |
| `messages` | array | Obligatorio. Mensajes de OpenAI: `role`, `content` (una cadena, un array de partes de contenido, o `null` en un turno del asistente con llamadas a herramientas) y, opcionalmente, `tool_calls`, `tool_call_id` y `name`. |
| `stream` | boolean | `true` para server-sent events. Consulta [Streaming](../guides/streaming-and-tools.md#streaming). |
| `temperature` | number | Temperatura de muestreo. |
| `max_tokens` | integer | Límite de tokens de salida. Solo se respeta en el carril Vertex nativo. |
| `response_format` | object | `{"type": "text" \| "json_object" \| "json_schema", "json_schema": {...}}`. Solo en el carril estándar; en el carril Vertex nativo usa `vertex.response_schema`. |
| `tools` | array | Herramientas de función de OpenAI. Consulta [Llamadas a herramientas](../guides/streaming-and-tools.md#tool-calling). |
| `tool_choice` | string or object | Solo se respeta en el carril Vertex nativo. |
| `routing_strategy` | string | `static` o `jev`. Consulta [Anular la decisión por petición](../guides/jev-router.md#override-the-decision-per-request). |
| `vertex` | object | Funcionalidades de Vertex nativo: `cached_content`, `media_uris`, `response_schema`, `thinking_config`. Selecciona el carril Vertex nativo; consulta [Detección de carril](../overview/architecture.md#lane-detection) y la [guía de Vertex nativo](../guides/native-vertex.md). |
| `jev` | object | Preguntas tipadas para el carril Jev: `questions`, `state` opcional, `extract` opcional. Consulta la [guía del carril Jev](../guides/jev-lane.md). |

El gateway acepta cualquier otro campo sin error, pero reenvía muy pocos:

- El **carril estándar** envía al proveedor `messages`, `tools`, `temperature`,
  `response_format` y un `reasoning_effort` de OpenAI. Descarta `max_tokens`, `tool_choice` y
  cualquier otro campo, como `top_p`, `stop` o `seed`.
- El **carril Vertex nativo** envía `messages`, `tools`, `tool_choice`, `temperature`,
  `max_tokens` y el bloque `vertex`. Consulta
  [Otros campos de la petición](../guides/native-vertex.md#other-request-fields).

[Limitaciones](./limitations-roadmap.md#request-fields) lista lo que falta.

### Respuesta {#response}

Una petición sin streaming devuelve un `chat.completion`:

```json
{
  "id": "chatcmpl-3f1c9a52-7d4e-4b8a-9c1e-2a6b0f9d8e71",
  "object": "chat.completion",
  "created": 1790620000,
  "model": "gemini-3.5-flash-lite",
  "choices": [{
    "index": 0,
    "message": { "role": "assistant", "content": "Hello!" },
    "finish_reason": "stop"
  }],
  "usage": { "prompt_tokens": 9, "completion_tokens": 3, "total_tokens": 12 }
}
```

- `id` es `chatcmpl-` seguido del id de la petición: el encabezado `x-synapse-message`, o un UUID
  generado.
- `model` es el modelo del tramo que sirvió la petición, no el alias de la ruta.
- `finish_reason` es `stop`, `length`, `tool_calls` o `content_filter`. Una llamada a
  herramienta tiene `content: null` y un array `tool_calls`. `content_filter` significa que el
  escaneo de salida bloqueó la respuesta, y `content` es `""`.
- Una respuesta de [extracción híbrida](../guides/jev-lane.md#hybrid-extraction) añade un objeto
  `jev` con `answers`, `survivors` y `degraded`.

### Respuesta con streaming {#streaming-response}

Con `"stream": true`, la respuesta es `text/event-stream`. Cada evento es un
`chat.completion.chunk` en una línea `data:`, y el stream termina con `data: [DONE]`:

```text
data: {"id":"chatcmpl-...","object":"chat.completion.chunk","created":0,"model":"gemini-3.5-flash-lite","choices":[{"index":0,"delta":{"content":"Hel"},"finish_reason":null}]}

data: {"id":"chatcmpl-...","object":"chat.completion.chunk","created":0,"model":"gemini-3.5-flash-lite","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}

data: [DONE]
```

Los fragmentos no llevan `usage` y `created` siempre es `0`. Un fallo después del primer
fragmento llega como un evento cuyo cuerpo es un objeto de error, seguido de `[DONE]`; consulta
[Fallback durante el streaming](../guides/streaming-and-tools.md#fallback-while-streaming).

Cuando el escaneo de salida bloquea una respuesta en streaming, el stream termina con un
fragmento cuyo `finish_reason` es `content_filter`, después un evento de error con `type`
`content_policy_violation`, `code` `content_blocked` y los `scanners` que bloquearon, y después
`[DONE]`. Consulta [Escaneo de salida](../configuration/guardrails-policy.md#streaming-responses).

## `POST /v1/embeddings` {#post-v1embeddings}

| Campo | Tipo | Descripción |
|---|---|---|
| `model` | string | Obligatorio. Un alias de embeddings de la tabla `embeddings` de `routes.toml`. |
| `input` | string or array of strings | Obligatorio. Los textos que se van a convertir en embeddings. |
| `dimensions` | integer | Opcional. Debe ser igual al `dimensions` del alias. |

La respuesta es una `list` de OpenAI de objetos `embedding`, en el orden de la entrada, con
`usage.prompt_tokens` y `usage.total_tokens`. El `model` de la respuesta es el alias. Consulta
[Enviar una petición](../guides/embeddings.md#send-a-request).

## Passthrough de Gemini {#gemini-passthrough}

`POST /v1beta/models/{model}:{action}`, `POST /v1/models/{model}:{action}` y
`POST /google/models/{model}:{action}` se comportan igual; los tres prefijos corresponden a las
versiones de la API que usan los SDK de Gemini de Google. El cuerpo es una petición de Gemini,
que se reenvía tal cual a Vertex AI con las credenciales del gateway.

- `{model}` es un alias de ruta o un nombre de modelo de Vertex AI, y `{action}` es la parte
  que va tras los últimos dos puntos, como `generateContent`, `streamGenerateContent` o
  `countTokens`.
- `streamGenerateContent` con `?alt=sse` transmite la respuesta en streaming.
- `generateContent` y `streamGenerateContent` escriben una fila en el registro de costes con el
  carril `passthrough`. Ante una respuesta `5xx`, `429` o `408` o un error de conexión, prueban
  los siguientes tramos `vertex` de la ruta que lista el modelo; consulta
  [Clientes del SDK de Gemini](../guides/native-vertex.md#gemini-sdk-clients).
- Las demás acciones se reenvían una sola vez, sin fila en el registro de costes.
- Un modelo que ninguna ruta lista se reenvía en un único intento solo con
  `SYNAPSE_PASSTHROUGH_MODELS=any`; por defecto recibe `404`.
- Las llamadas con streaming (`?alt=sse`) pueden durar hasta una hora. Todas las demás están
  acotadas por `SYNAPSE_REQUEST_TIMEOUT_SECS`.
- El estado y el cuerpo de Vertex AI se devuelven sin cambios. Un error de conexión en el último
  intento devuelve `502` con el código `upstream_error`. Si el gateway no tiene ningún proyecto
  de Vertex AI configurado, el endpoint devuelve `400`.

Todas las acciones salvo `countTokens` y `computeTokens` se escanean con la política de la
ruta o, si no, `default`, y un id de modelo con la política de cada ruta que lo incluye (consulta
[Qué se escanea](../configuration/guardrails-policy.md#what-is-scanned)).
Con `SYNAPSE_PASSTHROUGH_MODELS=routed` (el valor por defecto), un modelo que no es ni un alias
de ruta ni un tramo `vertex` recibe `404`. Los errores del propio gateway en este endpoint usan
la forma de error de Google.

## Passthrough de Jev {#jev-passthrough}

`POST /typesafe/v1/systemone` reenvía tal cual un cuerpo `{state, questions}` de TypeSafe System
One con la `TYPESAFE_API_KEY` del gateway. El cuerpo debe ser un objeto JSON; si no tiene
`model`, el gateway fija `jev-latest`. El estado y el cuerpo de TypeSafe se devuelven sin
cambios, y el uso se anota en el registro de costes con el carril `passthrough`. Sin
`TYPESAFE_API_KEY`, el endpoint devuelve `400`. Las peticiones se escanean con la política
`default`. Consulta
[Passthrough de Jev](../guides/jev-lane.md#jev-passthrough).

## Registro de agentes A2A {#a2a-agent-registry}

El binario del gateway sirve un registro en memoria de agentes A2A, cargado al arrancar desde
`a2a.toml` (consulta [Claves de configuración](./configuration-keys.md#a2atoml)). Cada
instancia del gateway tiene su propio registro. Los endpoints de administración se sirven en
`SYNAPSE_ADMIN_ADDR` (por defecto `127.0.0.1:8081`), no en el puerto de la API, salvo que se
configure como `shared`.

### `POST /internal/a2a/agents` {#post-internala2aagents}

Registra un agente. Todos los campos salvo `ttl_seconds` son obligatorios:

```json
{
  "id": "invoice-agent",
  "name": "Invoice agent",
  "description": "Extracts line items from invoices",
  "endpoint_url": "https://agents.example.com/invoice",
  "card_url": "https://agents.example.com/invoice/.well-known/agent-card.json",
  "tags": ["finance"],
  "card": { "name": "Invoice agent", "skills": [] },
  "ttl_seconds": 3600
}
```

Devuelve `204`. Si el id ya está registrado, se conserva la entrada existente y la petición
devuelve igualmente `204`. Con `ttl_seconds`, el agente desaparece del catálogo ese número de
segundos después del registro.

### `DELETE /internal/a2a/agents/{id}` {#delete-internala2aagentsid}

Elimina el agente. Devuelve `204`, exista o no el id.

:::warning
Los endpoints `/internal/` no tienen autenticación. Bloquéalos en tu proxy; consulta
[Los endpoints de administración de A2A están abiertos](../operating/security.md#the-a2a-admin-endpoints-are-open).
:::

### `GET /.well-known/a2a-agent-catalog.json` {#get-well-knowna2a-agent-catalogjson}

```json
{
  "version": "1.0",
  "agents": [{
    "id": "invoice-agent",
    "name": "Invoice agent",
    "description": "Extracts line items from invoices",
    "card_url": "https://agents.example.com/invoice/.well-known/agent-card.json",
    "endpoint_url": "https://agents.example.com/invoice",
    "tags": ["finance"]
  }]
}
```

### `GET /a2a/agents/{id}/.well-known/agent-card.json` {#get-a2aagentsidwell-knownagent-cardjson}

Devuelve la agent card del agente tal como se registró, o `404` con el cuerpo vacío.

### `GET /a2a/agents/{id}/resolve` {#get-a2aagentsidresolve}

Devuelve `{"id", "endpoint_url", "card_url", "card"}` para el agente, o `404` con el cuerpo
vacío.

## Métricas {#metrics}

`GET /metrics` y `GET /` en el puerto de métricas devuelven el formato de texto de Prometheus.
Consulta [Métricas](../operating/metrics.md) y el [catálogo de métricas](./metrics-catalogue.md).
