---
sidebar_position: 6
title: Embeddings
description: Sirve POST /v1/embeddings compatible con OpenAI mediante tramos de fallback con dimensiones fijadas en Vertex AI y OpenAI, con contabilidad de costes por tenant.
---

Usa el endpoint de embeddings de Synapse cuando construyas o consultes un índice vectorial y
quieras el mismo enrutamiento, fallback y contabilidad de costes por tenant que obtienes con
el chat. Los clientes llaman a `POST /v1/embeddings`, compatible con OpenAI, con un alias;
Synapse lo sirve desde un modelo de embeddings de Vertex AI o compatible con OpenAI y
garantiza que todos los tramos devuelven vectores de la misma longitud, de modo que una
conmutación nunca escribe vectores incompatibles en tu índice.

## Definir un alias {#define-an-alias}

Los alias de embeddings viven en `routes.toml`, junto a tus rutas de chat, en una tabla
`embeddings` aparte. Cada alias declara el tamaño de su vector en `dimensions` y una lista
ordenada de tramos:

```toml
[embeddings."embed"]
dimensions = 768
legs = [
  { provider = "vertex", model = "text-embedding-004" },
  { provider = "openai", model = "text-embedding-3-small" },
]
```

Synapse fija `dimensions` en todos los tramos: envía `outputDimensionality` a Vertex AI y
`dimensions` a OpenAI, así que los dos tramos de arriba devuelven 768 floats, aunque
`text-embedding-3-small` devuelve 1,536 por defecto. Todos los modelos de un alias deben
admitir reducir su salida a ese tamaño; los modelos antiguos de tamaño fijo, como
`textembedding-gecko` o `text-embedding-ada-002`, no pueden formar parte de un alias con
dimensiones fijadas. [Alias de embeddings](../configuration/routes.md#embedding-aliases)
enumera las reglas de validación.

:::warning
Los vectores de modelos distintos no son comparables, aunque tengan la misma longitud. Un
tramo de fallback evita que tu índice se rompa, pero sus vectores están en un espacio distinto
del de tu modelo principal. Si la calidad de la similitud importa más que la disponibilidad,
usa un alias de un solo tramo, o registra qué modelo produjo cada vector y vuelve a generar
los embeddings después de una conmutación.
:::

## Proveedores {#providers}

Los tramos de embeddings admiten dos proveedores:

- **`vertex`** llama al endpoint `:predict` de Vertex AI en el proyecto de
  `VERTEX_PROJECT_ID` y la ubicación de `VERTEX_LOCATION` (por defecto `global`), con las
  mismas credenciales que el chat. La `region` de un tramo no se usa para los embeddings.
- **`openai`** llama a `POST <OPENAI_BASE_URL>/embeddings` con `OPENAI_API_KEY`. Apunta
  `OPENAI_BASE_URL` a cualquier servidor de embeddings compatible con OpenAI para usarlo en su
  lugar.

La falta de una credencial para un proveedor referenciado detiene el gateway al arrancar con
la validación estricta; la validación permisiva descarta esos tramos. Consulta
[Proveedores](../configuration/providers.md#embedding-aliases).

## Enviar una petición {#send-a-request}

```bash
curl -s http://localhost:8080/v1/embeddings \
  -H "Content-Type: application/json" \
  -H "x-synapse-tenant: my-team" \
  -d '{ "model": "embed", "input": ["hello world", "second chunk"] }'
```

La respuesta tiene un vector de 768 floats por entrada, aquí abreviado:

```json
{
  "object": "list",
  "data": [
    { "object": "embedding", "index": 0, "embedding": [0.0123, -0.0456, 0.0311] },
    { "object": "embedding", "index": 1, "embedding": [0.0789, 0.0012, -0.0204] }
  ],
  "model": "embed",
  "usage": { "prompt_tokens": 7, "total_tokens": 7 }
}
```

- `input` es una cadena o un array de cadenas. Synapse divide los arrays grandes en lotes de
  250 para Vertex AI y de 2,048 para OpenAI, y devuelve los vectores en el orden de entrada.
- `dimensions` es opcional. Si lo envías, debe ser igual a las `dimensions` del alias, o la
  petición falla con `400`: el tamaño del vector lo decide el alias, no el cliente.
- `model` en la respuesta es el alias, no el modelo que la sirvió.
- Un alias desconocido devuelve `404` con el código de error `model_not_found`. Los alias de
  embeddings no aparecen en `GET /v1/models`, que solo enumera las rutas de chat.

## Fallback {#fallback}

Synapse prueba los tramos en orden. Se omite un tramo cuyo proveedor no ha configurado este
proceso, y cualquier error, incluido un `4xx`, pasa al siguiente tramo. Si una entrada grande
necesita varios lotes y uno falla, falla todo el tramo y el siguiente tramo vuelve a generar
los embeddings de todas las entradas, así que una respuesta nunca mezcla vectores de dos
modelos. Cuando fallan todos los tramos, se devuelve el error del último, normalmente `502`
con el código de error `upstream_error`.

Las llamadas de embeddings no tienen reintentos propios; cada tramo tiene un intento, limitado
por `SYNAPSE_REQUEST_TIMEOUT_SECS`.

## Coste {#cost}

El uso de embeddings solo cuenta tokens de entrada. Synapse lo valora con el precio `input` de
la entrada `provider:model` del tramo en `pricing.toml`:

```toml
"vertex:text-embedding-004" = { input = 0.025, output = 0.0 }
"openai:text-embedding-3-small" = { input = 0.02, output = 0.0 }
```

A diferencia del chat, un modelo de embeddings sin entrada no es gratuito: se valora a
`SYNAPSE_EMBED_DEFAULT_INPUT_PRICE_PER_MTOK` (por defecto `0.10` USD por 1,000,000 de
tokens), de modo que el uso nunca se registra en silencio con coste cero. Consulta
[Precios](../configuration/pricing.md#how-cost-is-computed).

El número de tokens lo proporciona el proveedor: el `statistics.token_count` de cada entrada
en Vertex AI y `usage.total_tokens` en OpenAI.

## Atribución y registro de costes {#attribution-and-the-ledger}

Las peticiones de embeddings aceptan los mismos encabezados de atribución que el chat, como
`x-synapse-tenant` y `x-synapse-workspace`; consulta
[Atribución por tenant](tenant-attribution.md). Cada petición correcta escribe una fila en el
registro con el carril `embedding`, el proveedor y el modelo del tramo que la sirvió, y
`output_tokens` a 0. En los eventos publicados en Pub/Sub o SNS, la fila tiene
`op = "embedding"`. Consulta [Registro de costes](cost-ledger.md).

Cada elemento de `input` se escanea con la `policy` del alias, si no con `default`, antes del
primer tramo; consulta [Qué se escanea](../configuration/guardrails-policy.md#what-is-scanned).

## Métricas {#metrics}

| Métrica | Etiquetas | Descripción |
|---|---|---|
| `synapse_embeddings_total` | `route`, `model`, `provider` | Peticiones de embeddings servidas. `route` es el alias. |
| `synapse_embedding_duration_seconds` | `route`, `model`, `provider` | Latencia del tramo que sirvió la petición. |

## Embeddings en el mismo proceso {#in-process-embeddings}

Las aplicaciones que embeben el gateway llaman directamente a `Gateway::embed`; consulta
[Embeber Synapse como biblioteca](embedding-as-library.md#embeddings).
