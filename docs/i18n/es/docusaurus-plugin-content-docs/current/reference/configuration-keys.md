---
sidebar_position: 2
title: Claves de configuración
description: Todas las claves de todos los archivos de configuración del gateway, con su tipo, si son obligatorias y su valor por defecto.
---

Esta página lista todas las claves que el gateway lee de sus archivos TOML, en una sola página
para consultarlas. Las [páginas de configuración](../configuration/environment-variables.md)
explican qué hacen las claves y cómo se combinan; cada sección de abajo enlaza a su página.

El gateway lee cada archivo una sola vez al arrancar; reinícialo para aplicar un cambio. Las
claves que el gateway no conoce se ignoran sin error, así que una clave opcional mal escrita no
tiene ningún efecto sin que nada te avise.

## Archivos {#files}

| Archivo | Variable de ruta | Ruta por defecto | Si falta el archivo |
|---|---|---|---|
| `routes.toml` | `SYNAPSE_ROUTES_PATH` | `config/routes.toml` | El gateway no arranca. |
| `pricing.toml` | `SYNAPSE_PRICING_PATH` | `config/pricing.toml` | El gateway no arranca. |
| `guardrails.toml` | `SYNAPSE_GUARDRAILS_PATH` | `config/guardrails.toml` | El gateway no arranca, salvo con `SYNAPSE_GUARDRAILS=optional`, que arranca con los guardrails desactivados. |
| `ai_task_types.toml` | `SYNAPSE_AI_TASK_TYPES_PATH` | `config/ai_task_types.toml` | Todas las peticiones registran el tipo de tarea de IA `simple`. |
| `a2a.toml` | `SYNAPSE_A2A_PATH` | `config/a2a.toml` | El registro A2A arranca vacío. |

Un archivo que existe pero no se puede analizar, o que no supera la validación, detiene el
gateway al arrancar con un mensaje que indica el problema. Las variables de entorno se listan en
[Variables de entorno](../configuration/environment-variables.md).

## `routes.toml` {#routestoml}

Rutas de chat y alias de embeddings. Consulta [Rutas](../configuration/routes.md).

El archivo debe contener una tabla `routes`, aunque todos los alias que necesites sean alias de
embeddings. Un archivo con solo tablas `[embeddings.*]` no se carga.

### `[routes."<alias>"]` {#routesalias}

| Clave | Tipo | Obligatoria | Por defecto | Descripción |
|---|---|---|---|---|
| `legs` | array of leg tables | Sí, en rutas estáticas | — | La cadena de fallback. Debe estar ausente o vacía en una ruta `jev`. |
| `strategy` | string | No | `"static"` | `"static"` o `"jev"`. |
| `policy` | string | No | la política `default` | Nombre de la política de guardrails de `guardrails.toml`. |
| `jev_router` | table | Sí, en rutas `jev` | — | Ajustes de la decisión. Consulta [más abajo](#routesaliasjev_router). |
| `tiers` | array of tier tables | Sí, en rutas `jev` | — | De 2 a 10 niveles, del más fácil al más difícil. Consulta [más abajo](#routesaliastiers). |

### Tabla de tramo {#leg-table}

Se usa en `legs`, en los `legs` de cada nivel y en los alias de embeddings.

| Clave | Tipo | Obligatoria | Por defecto | Descripción |
|---|---|---|---|---|
| `provider` | string | Sí | — | `vertex`, `openai`, `qwen`, `oai_compat` o `typesafe`. Los tramos de embeddings admiten `vertex` y `openai`. Consulta [Proveedores](../configuration/providers.md). |
| `model` | string | Sí | — | El nombre del modelo del proveedor, enviado tal cual. |
| `region` | string | No | `VERTEX_LOCATION` | Ubicación de Vertex para este tramo en el carril Vertex nativo. Se ignora en los demás casos. |

### `[routes."<alias>".jev_router]` {#routesaliasjev_router}

| Clave | Tipo | Obligatoria | Por defecto | Descripción |
|---|---|---|---|---|
| `default_tier` | string | Sí | — | Nivel que se usa cuando Jev no puede decidir. Debe nombrar un nivel. |
| `model` | string | No | `"jev-latest"` | Modelo de Jev que toma la decisión. |
| `timeout_ms` | integer | No | `400` | Tiempo límite de la decisión en milisegundos. Mayor que 0. |
| `min_confidence` | float | No | `0.5` | Por debajo de esta confianza, sirve `default_tier`. De 0 a 1. |
| `reasoning_threshold` | float | No | `0.7` | A partir de este valor (incluido), el esfuerzo sube un paso. De 0 a 1. |

Consulta [Claves de `jev_router`](../configuration/routes.md#jev_router-keys).

### `[[routes."<alias>".tiers]]` {#routesaliastiers}

| Clave | Tipo | Obligatoria | Por defecto | Descripción |
|---|---|---|---|---|
| `name` | string | Sí | — | Único, no vacío, ASCII imprimible. |
| `description` | string | Sí | — | El trabajo para el que sirve este nivel. No vacío. |
| `effort` | string | Sí | — | `none`, `minimal`, `low`, `medium`, `high`, `xhigh` o `max`. |
| `legs` | array of leg tables | Sí | — | Al menos un tramo; sin tramos `typesafe`. |

Consulta [Claves de nivel](../configuration/routes.md#tier-keys).

### `[embeddings."<alias>"]` {#embeddingsalias}

| Clave | Tipo | Obligatoria | Por defecto | Descripción |
|---|---|---|---|---|
| `dimensions` | integer | Sí | — | Tamaño del vector de salida. Mayor que 0. |
| `legs` | array of leg tables | Sí | — | Al menos un tramo. |
| `policy` | string | No | la política `default` | Nombre de la política de guardrails de `guardrails.toml`. |

Consulta [Alias de embeddings](../configuration/routes.md#embedding-aliases).

## `pricing.toml` {#pricingtoml}

Precios para el registro de costes. Consulta [Precios](../configuration/pricing.md).

Cada clave de nivel superior es una cadena `"<provider>:<model>"`, y su valor es una tabla:

| Clave | Tipo | Obligatoria | Descripción |
|---|---|---|---|
| `input` | float | Sí | USD por 1,000,000 de tokens de entrada. |
| `output` | float | Sí | USD por 1,000,000 de tokens de salida. |

```toml
"openai:gpt-4o-mini" = { input = 0.15, output = 0.60 }
```

Una clave de nivel superior cuyo valor no sea una tabla así detiene el gateway. Un archivo vacío
es válido.

## `guardrails.toml` {#guardrailstoml}

Políticas de guardrails con nombre. Consulta
[Política de guardrails](../configuration/guardrails-policy.md).

### `[guardrails.<name>]` {#guardrailsname}

| Clave | Tipo | Obligatoria | Por defecto | Descripción |
|---|---|---|---|---|
| `scanners` | array | Sí | — | Nombres de escáneres, o tablas de escáner. |
| `mode` | string | No | `"block"` | `"block"` o `"observe"`. |

### Tabla de escáner {#scanner-table}

Un escáner es un nombre sin más, como `"secrets"`, o una tabla:

| Clave | Tipo | Usada por | Descripción |
|---|---|---|---|
| `type` | string | todos los escáneres | Obligatoria. El nombre del escáner: `prompt_injection`, `secrets`, `pii`, `invisible_text`, `role_override`, `token_limit`, `ban_substrings` o `script_mix`. |
| `max_chars` | integer | `token_limit` | Obligatoria para `token_limit`. Longitud máxima de la entrada en caracteres. |
| `substrings` | array of strings | `ban_substrings` | Obligatoria para `ban_substrings`, no vacía. |
| `severity` | string | `ban_substrings` | `block` (por defecto), `warn` o `info`. |
| `threshold` | integer | `script_mix` | Caracteres fuera del alfabeto dominante antes de marcar la petición. Por defecto `2`. |

Consulta [Escáneres](../configuration/guardrails-policy.md#scanners).

## `ai_task_types.toml` {#ai_task_typestoml}

Asigna alias de ruta al tipo de tarea de IA de las filas del registro de costes. Consulta
[Tipos de tarea de IA](../guides/tenant-attribution.md#ai-task-types).

Cada clave de nivel superior es un nombre de tipo de tarea, y su valor es un array de alias de
ruta, de chat o de embeddings:

```toml
conversation = ["chat", "support-bot"]
extraction = ["invoice-extract"]
```

Un alias listado bajo dos tipos de tarea detiene el gateway. Los alias no listados se resuelven a
`simple`.

## `a2a.toml` {#a2atoml}

Agentes que se registran en el registro A2A del gateway al arrancar. Cada agente es una tabla
`[[a2a_agents]]`:

| Clave | Tipo | Obligatoria | Por defecto | Descripción |
|---|---|---|---|---|
| `id` | string | Sí | — | Id en el registro, usado en los paths `/a2a/agents/{id}/...`. |
| `name` | string | Sí | — | Nombre visible en el catálogo. |
| `description` | string | Sí | — | Descripción en el catálogo. |
| `endpoint_url` | string | Sí | — | URL absoluta donde el agente sirve A2A. |
| `card_url` | string | Sí | — | URL absoluta de la agent card. El gateway la descarga al arrancar. |
| `tags` | array of strings | No | `[]` | Etiquetas del catálogo. |
| `ttl_seconds` | integer | No | sin caducidad | Segundos tras el arranque al cabo de los cuales el agente sale del catálogo. |

```toml
[[a2a_agents]]
id = "invoice-agent"
name = "Invoice agent"
description = "Extracts line items from invoices"
endpoint_url = "https://agents.example.com/invoice"
card_url = "https://agents.example.com/invoice/.well-known/agent-card.json"
tags = ["finance"]
```

Al arrancar, el gateway descarga la agent card de cada agente desde `card_url`, con un tiempo de
espera de 15 segundos. Los errores de conexión y las respuestas `5xx` y `429` se reintentan dos
veces con backoff. Un agente cuya card no se puede descargar se registra en el log y se omite, y
el gateway arranca sin él; un `id` duplicado también se omite. El registro y sus endpoints se
describen en [Registro de agentes A2A](./http-api.md#a2a-agent-registry).
