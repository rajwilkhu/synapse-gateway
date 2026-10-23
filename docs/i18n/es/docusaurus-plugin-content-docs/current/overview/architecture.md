---
sidebar_position: 2
title: Arquitectura
description: Cómo lleva Synapse una petición de chat completion a través de los guardrails, la planificación de la ruta, uno de tres carriles y una cadena de fallback de proveedores.
---

Synapse acepta peticiones de chat completion compatibles con OpenAI y sirve cada una a
través de uno de tres carriles de backend: el carril **estándar**, el carril **Vertex
nativo** o el carril **Jev**. Solo el cuerpo de la petición decide el carril, así que un
cliente activa las funcionalidades exclusivas de Vertex o las decisiones de Jev añadiendo
un bloque de extensión, sin un endpoint aparte.

## Flujo de una petición {#request-flow}

```text
client
  │  POST /v1/chat/completions   (model = route alias, x-synapse-tenant header)
  ▼
route lookup ─────────────► 404 model_not_found (unknown alias)
  │
  ▼
input guardrails (route policy; enforced in `block` mode or dry-run in `observe` mode)
  ├─► 1. in-process span scanners (`llm-guard`: block, `[REDACTED:...]` mask, or log)
  └─► 2. Jev semantic classifier (optional `noul` questions on post-mask input)
  │                        └─► 400 content_blocked (or 503 guard_unavailable)
  ▼
route planning             static route: its legs
  │                        strategy = "jev": the Jev router scores difficulty + reasoning
  ▼
lane detection
  ├─► standard lane        (genai: OpenAI, Qwen, oai_compat, Vertex; per-leg retries & breaker)
  ├─► native Vertex lane   (Vertex REST: :streamGenerateContent)
  └─► Jev lane             (TypeSafe System One questions or `jev.extract` hybrid extraction)
  │
  ▼
fallback chain             leg 1 → leg 2 → … until one succeeds
  │
  ▼
output guardrails          optional `output_scanners` (buffered or streaming holdback window)
  │
  ▼
response to the client (JSON or server-sent events)
  │
  ├─► cost ledger          tokens and cost per tenant (`standard`/`native`/`jev`/`guard`)
  └─► metrics & logs       OpenTelemetry `synapse_*` metrics + `synapse::*` tracing events
```

Cada alias de ruta en `routes.toml` corresponde a una lista ordenada de tramos (proveedor
más modelo). Synapse prueba los tramos en orden hasta que uno tiene éxito. En el carril
estándar, cualquier fallo pasa al siguiente tramo: una respuesta de error, un tiempo de
espera agotado antes del primer fragmento o un stream interrumpido. En el carril Vertex
nativo, solo una respuesta `5xx`, `429` o `408`, un error de conexión o un tiempo de espera
agotado hacen pasar al siguiente; cualquier otro `4xx` detiene la cadena. Una respuesta en
streaming solo puede hacer fallback hasta que su primer fragmento llega al cliente.
Consulta la [guía de cadenas de fallback](../guides/fallback-chains.md).

La escritura en el registro nunca bloquea la respuesta: si la cola del registro está llena,
el evento se descarta y se cuenta en `synapse_ledger_dropped_total`. Consulta la
[guía del registro de costes](../guides/cost-ledger.md) y el
[catálogo de métricas](../reference/metrics-catalogue.md). Para un recorrido paso a paso por
el código fuente, consulta [Pipeline de una petición](../internals/request-pipeline.md).

## Carriles {#lanes}

### Carril estándar {#standard-lane}

Las peticiones sin activadores de carril usan el carril estándar, que llama a los
proveedores a través del crate [`genai`](https://crates.io/crates/genai). Cualquier
proveedor con una API compatible con OpenAI puede aparecer en la cadena: OpenAI, Qwen
(DashScope) y vLLM, Ollama o TGI autoalojados a través del proveedor `oai_compat`. Los
tramos de Vertex también funcionan aquí, sin las funcionalidades exclusivas del carril
nativo. Consulta [Proveedores](../configuration/providers.md).

### Carril Vertex nativo {#native-vertex-lane}

Las peticiones que usan funcionalidades exclusivas de Vertex van al carril Vertex nativo,
que llama directamente al endpoint REST `:streamGenerateContent` de Vertex AI, tanto para
clientes con streaming como sin él; para un cliente sin streaming, Synapse consolida el
stream en una única respuesta. El formato de mensajes de OpenAI se traduce al de Vertex, y
se conservan estos campos del bloque `vertex` de la petición:

- **`cached_content`**: el nombre de un recurso `cachedContents`, para la caché de
  contexto.
- **`media_uris`**: URIs de Cloud Storage (`gs://`), adjuntadas como partes de archivo con
  el tipo MIME `video/mp4`.
- **`response_schema`**: un esquema JSON enviado como `generationConfig.responseSchema`
  para la decodificación restringida.
- **`thinking_config`**: se pasa tal cual como `generationConfig.thinkingConfig`.

Solo participan los tramos `vertex` de la ruta; los demás proveedores no pueden servir
estas funcionalidades. Si la ruta no tiene ningún tramo `vertex`, Synapse devuelve
`400 Bad Request` con el código de error `native_feature_unsupported` en lugar de descartar
las funcionalidades en silencio. Consulta la
[guía de Vertex nativo](../guides/native-vertex.md).

### Carril Jev {#jev-lane}

Las peticiones con un bloque `jev` que lleva preguntas tipadas van a los tramos `typesafe`
de la ruta. TypeSafe System One (Jev) evalúa las preguntas frente a un estado, que por
defecto son los `messages` de la petición, y devuelve decisiones estructuradas como
contenido del mensaje. Si todos los tramos `typesafe` fallan con un error reintentable, los
tramos restantes de la ruta responden como un chat completion normal, así que los clientes
deben gestionar ambas formas de respuesta. Una ruta con tramos `typesafe` devuelve
`400 Bad Request` a las peticiones sin bloque `jev`. Consulta la
[guía del carril Jev](../guides/jev-lane.md).

## Detección de carril {#lane-detection}

Synapse examina el cuerpo de la petición en este orden:

1. Un bloque `jev` con un mapa `questions` no vacío selecciona el carril **Jev**. Un bloque
   `vertex` en la misma petición se sigue aplicando si la cadena hace fallback a un tramo
   Vertex nativo.
2. Un bloque `vertex` con cualquiera de `cached_content`, `response_schema`,
   `thinking_config` o una entrada de `media_uris` que empiece por `gs://` selecciona el
   carril **Vertex nativo**.
3. Todo lo demás usa el carril **estándar**.

Por ejemplo, esta petición usa el carril Vertex nativo:

```json
{
  "model": "gemini-flash",
  "messages": [{ "role": "user", "content": "Summarise the video." }],
  "vertex": {
    "cached_content": "projects/my-gcp-project/locations/us/cachedContents/abc123",
    "media_uris": ["gs://my-bucket/video.mp4"],
    "response_schema": { "type": "object", "properties": { "summary": { "type": "string" } } }
  }
}
```

La detección de carril y la estrategia de enrutamiento son independientes. En una ruta
`strategy = "jev"`, el router Jev elige de qué nivel salen los tramos de la cadena, y el
carril sigue dependiendo del cuerpo de la petición: una petición Vertex nativa la sirve el
nivel más cercano que tenga un tramo `vertex`. Consulta la
[guía del router Jev](../guides/jev-router.md).
