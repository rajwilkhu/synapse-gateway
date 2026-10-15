---
sidebar_position: 3
title: Rutas
description: Referencia de routes.toml, que asigna los nombres de modelo que envían los clientes a cadenas de fallback de tramos de proveedor o a niveles de dificultad enrutados por Jev.
---

`routes.toml` define los nombres de modelo que pueden enviar tus clientes. Cada ruta asigna un
alias a los proveedores y modelos que lo sirven: o bien una lista ordenada de tramos o, con
`strategy = "jev"`, un conjunto de niveles de dificultad entre los que Jev elige en cada
petición.

El gateway lee el archivo de `SYNAPSE_ROUTES_PATH` (por defecto `config/routes.toml`) al
arrancar y se niega a iniciarse si falta o no es válido. Reinicia el gateway para aplicar los
cambios. `GET /v1/models` lista todos los alias.

## Rutas estáticas {#static-routes}

Una ruta estática es una tabla con el nombre de su alias y una lista de tramos:

```toml
[routes."chat"]
legs = [
  { provider = "vertex", model = "gemini-3.5-flash-lite", region = "us" },
  { provider = "openai", model = "gpt-4o-mini" },
]
```

Los clientes la seleccionan enviando `"model": "chat"`. Pon el alias entre comillas en el
nombre de la tabla cuando contenga caracteres distintos de letras, dígitos, `-` y `_`.

| Clave | Obligatoria | Descripción |
|---|---|---|
| `legs` | Sí, en rutas estáticas | La cadena de fallback: tramos que se prueban en orden hasta que uno tiene éxito. |
| `policy` | No | Nombre de la política de guardrails que se aplica. Sin ella, se aplica la política `default`. Consulta [Política de guardrails](guardrails-policy.md). |
| `strategy` | No | `static` (valor por defecto) o `jev`. Consulta [Rutas Jev](#jev-routes). |

### Claves de un tramo {#leg-keys}

| Clave | Obligatoria | Descripción |
|---|---|---|
| `provider` | Sí | `vertex`, `openai`, `qwen`, `oai_compat` o `typesafe`. Consulta [Proveedores](providers.md). |
| `model` | Sí | El nombre del modelo en el proveedor, enviado tal cual. También es la parte de modelo de la clave `provider:model` en `pricing.toml`. |
| `region` | No | Ubicación de Vertex para este tramo en el carril Vertex nativo, como `global`, `us` o `us-central1`. Se ignora en los demás carriles. |

## Fallback {#fallback}

Synapse prueba los tramos de una ruta en orden y devuelve la primera respuesta correcta; el
cliente nunca ve los intentos fallidos. Lo que cuenta como fallo depende del carril:

- En el **carril estándar**, cualquier fallo pasa al siguiente tramo: una respuesta de error,
  que no llegue el primer fragmento dentro de `SYNAPSE_REQUEST_TIMEOUT_SECS`, o un stream que
  se detiene o se corta.
- En el **carril Vertex nativo**, solo participan los tramos `vertex` de la ruta, y solo una
  respuesta `5xx`, `429` o `408`, un error de conexión o un tiempo de espera agotado hacen
  avanzar la cadena. Cualquier otro `4xx` la detiene.
- Una respuesta en **streaming** solo puede hacer fallback hasta que su primer fragmento llega
  al cliente.

Cuando fallan todos los tramos de una petición del carril estándar, el cliente recibe `502` con
el código de error `all_legs_failed` y un array `failures` que nombra cada tramo y su error. El
[tutorial de fallback](../get-started/tutorials/fallback-across-providers.md) recorre estos
casos, y la [guía de cadenas de fallback](../guides/fallback-chains.md) explica cómo diseñar
una cadena.

## Regiones {#regions}

La `region` de un tramo solo se aplica en el carril Vertex nativo. Un tramo Vertex nativo sin
`region` usa `VERTEX_LOCATION` (por defecto `global`). Úsala para fijar un modelo a la
ubicación que lo sirve, por ejemplo `global` para los modelos preview de Gemini, sin cambiar el
valor por defecto de todo el proceso. Las llamadas a Vertex del carril estándar siempre usan el
endpoint `global`.

Una caché de contexto de Vertex debe estar en la misma ubicación que el tramo que la usa.

## Rutas Jev {#jev-routes}

Una ruta con `strategy = "jev"` declara niveles de dificultad en lugar de tramos. En cada
petición, Synapse pregunta a TypeSafe Jev lo exigente que es la conversación, puntuada frente
a las descripciones de los niveles, y si necesita razonamiento paso a paso. El nivel más cercano
sirve la petición con su `effort` de razonamiento, subido un paso cuando es probable que haga
falta razonar.

Una ruta `jev` tiene una `strategy`, una tabla `jev_router` y una tabla
`[[routes."<alias>".tiers]]` por nivel:

```toml
[routes."auto"]
strategy = "jev"

[routes."auto".jev_router]
default_tier = "moderate"

[[routes."auto".tiers]]
name = "moderate"
description = "Everyday Q&A, summarising, simple extraction or code edits"
effort = "low"
legs = [{ provider = "vertex", model = "gemini-3.6-flash", region = "global" }]

# ... al menos un nivel más
```

Consulta el [ejemplo completo](#complete-example) para ver una ruta de tres niveles que
funciona.

Una ruta `jev` necesita `TYPESAFE_API_KEY` con la validación estricta de proveedores. Consulta
[Proveedores](providers.md#strict-and-lenient-validation) para saber qué hace sin ella la
validación permisiva.

### Claves de `jev_router` {#jev_router-keys}

| Clave | Valor por defecto | Descripción |
|---|---|---|
| `default_tier` | Obligatoria | Nivel que sirve cuando Jev agota el tiempo de espera, falla o responde con baja confianza. Debe nombrar un nivel. |
| `model` | `jev-latest` | Modelo de Jev que toma la decisión. Las filas de decisión del registro de costes se valoran como `typesafe:<model>`. |
| `timeout_ms` | `400` | Límite de tiempo de la llamada de decisión, en milisegundos. Debe ser mayor que 0. |
| `min_confidence` | `0.5` | Por debajo de esta confianza, sirve `default_tier`. Entre 0 y 1. |
| `reasoning_threshold` | `0.7` | Si la probabilidad de que la petición necesite razonamiento es igual o superior a este valor, el esfuerzo sube un paso. Entre 0 y 1. |

### Claves de un nivel {#tier-keys}

Declara de 2 a 10 tablas `[[routes."<alias>".tiers]]`, ordenadas del nivel más fácil al más
difícil.

| Clave | Descripción |
|---|---|
| `name` | Único, no vacío, ASCII imprimible (se permiten espacios). Se devuelve en el encabezado de respuesta `x-synapse-tier`. |
| `description` | El tipo de trabajo al que se adapta el nivel. Jev puntúa las peticiones frente a ella, así que describe el trabajo, nunca el modelo. |
| `effort` | Esfuerzo de razonamiento de los tramos del nivel: `none`, `minimal`, `low`, `medium`, `high`, `xhigh` o `max`. |
| `legs` | Los tramos del nivel, con la misma forma que los de una ruta estática. Cualquier proveedor excepto `typesafe`. |

### Esfuerzo {#effort}

En el carril estándar, Synapse pasa el esfuerzo de un nivel al crate `genai` como su opción
`ReasoningEffort`, y genai lo traduce para cada proveedor. En el carril Vertex nativo, Synapse
establece `thinkingBudget` por sí mismo:

| Esfuerzo | `openai`, `qwen`, `oai_compat`: `reasoning_effort` | `vertex`, carril estándar, Gemini 3: `thinkingLevel` | `vertex`, carril nativo: `thinkingBudget` |
|---|---|---|---|
| `none` | no se envía | no se envía | no se envía |
| `minimal` | `minimal` | `MINIMAL` | 512 |
| `low` | `low` | `LOW` | 1024 |
| `medium` | `medium` | `MEDIUM` | 4096 |
| `high` | `high` | `HIGH` | 8192 |
| `xhigh` | `xhigh` | `HIGH` | 16384 |
| `max` | `xhigh` | `HIGH` | 24576 |

En el carril estándar, los modelos de Vertex cuyo nombre no contiene `gemini-3` reciben en su
lugar un `thinkingBudget` de genai: 1000 tokens para `minimal` y `low`, 8000 para `medium`, y
24000 para `high`, `xhigh` y `max`.

Con `none`, se aplica el valor por defecto del modelo; en algunos modelos Gemini ese valor por
defecto es el razonamiento dinámico, que puede costar más que `minimal`. El esfuerzo del propio
cliente siempre gana en su carril: `reasoning_effort` en el carril estándar, traducido de la
misma forma, y `vertex.thinking_config` en el carril Vertex nativo.

La [guía del router Jev](../guides/jev-router.md#effort) explica cómo elegir un esfuerzo para
cada nivel.

### Fallback entre niveles {#tier-fallback}

Primero se ejecutan los tramos del nivel elegido, luego los de cada nivel más difícil y después
los de cada nivel más fácil. Una petición con funcionalidades de Vertex nativo solo usa tramos
`vertex`; si el nivel elegido no tiene ninguno, la sirve el nivel más cercano que sí los tenga,
empezando por los más difíciles. Los clientes pueden enviar `"routing_strategy": "static"` para
omitir la decisión y empezar en `default_tier`.

Una ruta `jev` devuelve `400` a las peticiones que llevan un bloque `jev` (`questions` o
`extract`).

## Alias de embeddings {#embedding-aliases}

Los alias de embeddings para `POST /v1/embeddings` están en el mismo archivo, bajo una tabla
`embeddings` aparte y con un tamaño de salida declarado:

```toml
[embeddings."embed"]
dimensions = 768
legs = [{ provider = "vertex", model = "text-embedding-004" }]
```

`dimensions` debe ser mayor que 0 y `legs` no puede estar vacío. Los tramos de embeddings
admiten los proveedores `vertex` y `openai`. Consulta la
[guía de embeddings](../guides/embeddings.md) para el fallback, el coste y la atribución.

## Ejemplo completo {#complete-example}

Este archivo combina una ruta de un solo tramo, una ruta con dos proveedores y una política de
guardrails, y una ruta Jev de tres niveles con todas las claves de `jev_router` escritas:

```toml title="config/routes.toml"
[routes."gemini-flash"]
legs = [
  { provider = "vertex", model = "gemini-3.5-flash-lite", region = "us" },
]

[routes."chat"]
policy = "strict"
legs = [
  { provider = "vertex", model = "gemini-3.5-flash-lite", region = "us" },
  { provider = "openai", model = "gpt-4o-mini" },
]

[routes."auto"]
strategy = "jev"

[routes."auto".jev_router]
default_tier = "moderate"
model = "jev-latest"
timeout_ms = 400
min_confidence = 0.5
reasoning_threshold = 0.7

[[routes."auto".tiers]]
name = "trivial"
description = "Greetings, chit-chat, one-line lookups or rewrites"
effort = "none"
legs = [{ provider = "vertex", model = "gemini-3.5-flash-lite", region = "us" }]

[[routes."auto".tiers]]
name = "moderate"
description = "Everyday Q&A, summarising, simple extraction or code edits"
effort = "low"
legs = [{ provider = "vertex", model = "gemini-3.6-flash", region = "global" }]

[[routes."auto".tiers]]
name = "hard"
description = "Multi-step analysis, maths or proofs, non-trivial code or debugging"
effort = "medium"
legs = [{ provider = "vertex", model = "gemini-3.1-pro-preview", region = "global" }]
```

## Validación {#validation}

El gateway comprueba el archivo al arrancar y se detiene con un mensaje que nombra la ruta
cuando:

- una ruta estática no tiene la clave `legs`, o declara `tiers` sin `strategy = "jev"`;
- `strategy` no es ni `static` ni `jev`;
- una ruta `jev` tiene `legs` no vacío, no tiene tabla `jev_router`, tiene menos de 2 o más de
  10 niveles, o tiene un `default_tier` que no es uno de sus niveles;
- un nivel tiene un nombre vacío o no ASCII, un nombre duplicado, una descripción vacía, ningún
  tramo, un tramo `typesafe` o un `effort` desconocido;
- `min_confidence` o `reasoning_threshold` están fuera del rango de 0 a 1, o `timeout_ms` es 0.

:::warning
Las claves desconocidas se ignoran, no se rechazan. Una clave mal escrita, como `polcy`, no
tiene ningún efecto y no produce ningún aviso.
:::

Las credenciales de los proveedores se comprueban después de cargar el archivo; consulta
[Proveedores](providers.md#strict-and-lenient-validation).

## Política de embeddings {#embedding-policy}

Un alias de embeddings puede nombrar una política de guardrails. Sin ella, los embeddings usan
`default`:

```toml title="config/routes.toml"
[routes."chat"]
legs = [{ provider = "vertex", model = "gemini-3.5-flash-lite", region = "us" }]

[embeddings."docs-embed"]
dimensions = 768
policy = "embeddings"
legs = [{ provider = "vertex", model = "text-embedding-004" }]
```

La política nombrada debe estar definida en `guardrails.toml`; con el valor por defecto
`SYNAPSE_GUARDRAILS=required`, de lo contrario el gateway se niega a arrancar.

## Passthrough de Gemini por alias {#gemini-passthrough-by-alias}

Un cliente del SDK de Gemini puede nombrar un alias de ruta como su modelo. El gateway recorre
los tramos `vertex` iniciales de esa ruta, envía a Vertex el modelo propio de cada tramo y
escanea con la política de la ruta. Las rutas con `strategy = "jev"` no están disponibles en el
passthrough. Un alias cuya ruta no tiene tramos `vertex` iniciales recibe `400`.
