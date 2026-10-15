---
sidebar_position: 2
title: Carril Jev
description: Haz preguntas tipadas a TypeSafe System One (Jev) a través de la API de chat completions, recurre a modelos de chat cuando Jev no esté disponible, y evalúa candidatos y extrae datos en una sola llamada.
---

Usa el carril Jev cuando necesites una decisión en lugar de prosa: qué equipo debe atender un
ticket, si un mensaje es urgente, cuánto se ajusta un documento a una descripción. Envías
preguntas tipadas en un bloque `jev` y TypeSafe System One (Jev) las responde sobre la
conversación con respuestas estructuradas y puntuadas. Usa la extracción híbrida cuando
quieras que Jev evalúe un conjunto de candidatos y que un modelo de chat extraiga datos solo
de los que lo superan, en una sola petición.

## Configurar una ruta {#set-up-a-route}

Una ruta sirve el carril Jev a través de sus tramos `typesafe`. Añade tramos de chat después
de ellos si quieres que la ruta siga respondiendo cuando Jev no esté disponible:

```toml
[routes."ticket-triage"]
legs = [
  { provider = "typesafe", model = "jev-latest" },
  { provider = "vertex", model = "gemini-3.5-flash-lite", region = "us" },
]
```

Los tramos `typesafe` necesitan `TYPESAFE_API_KEY`; consulta
[Proveedores](../configuration/providers.md#typesafe). Una ruta con tramos `typesafe` solo
acepta peticiones que lleven un bloque `jev` con preguntas, y devuelve `400` a cualquier otra
petición.

## Hacer preguntas tipadas {#ask-typed-questions}

Añade un bloque `jev` con un mapa `questions` no vacío a una petición de chat completion. Cada
entrada asocia el nombre de una pregunta con su definición: un `type` (`choice`, `score` o
`noul`), las `instructions` y, cuando el tipo lo requiere, `criteria`. Synapse reenvía las
preguntas a Jev literalmente; la documentación de TypeSafe describe cada tipo.

```json
{
  "model": "ticket-triage",
  "messages": [{ "role": "user", "content": "The invoice for March charged us twice." }],
  "jev": {
    "questions": {
      "department": {
        "type": "choice",
        "instructions": "Which team should handle this ticket?",
        "criteria": {
          "billing": "Invoices, payments, refunds",
          "technical": "Errors, outages, integrations"
        }
      },
      "is_urgent": { "type": "noul", "instructions": "Is this urgent?" }
    }
  }
}
```

Jev evalúa las preguntas sobre un **estado**. Por defecto, el estado son los `messages` de la
petición, serializados como una cadena JSON. Establece `jev.state` a una cadena propia para
evaluar otra cosa, como un documento. El estado y las preguntas comparten el presupuesto de
entrada de Jev, así que mantén ambos centrados.

Los guardrails de entrada se aplican a las peticiones del carril Jev como a cualquier petición
de chat; consulta [Política de guardrails](../configuration/guardrails-policy.md).

## Leer las respuestas {#read-the-answers}

Una respuesta correcta es un `chat.completion` normal:

- `choices[0].message.content` es una cadena JSON. Analízala para obtener el mapa `answers` de
  Jev, indexado por nombre de pregunta. La forma de cada respuesta depende del tipo de su
  pregunta.
- `model` es la build de Jev que respondió.
- `usage` lleva el número de tokens de entrada y de salida de Jev.

## Cuando Jev falla {#when-jev-fails}

Synapse prueba los tramos `typesafe` de la ruta en orden:

- Un fallo reintentable, es decir, una respuesta `429`, `408` o `5xx` o un error de conexión,
  pasa al siguiente tramo `typesafe`.
- Un fallo no reintentable, como unas preguntas mal formadas, detiene la petición con `400` y
  el mensaje de error de TypeSafe. Los tramos de chat no pueden arreglar una pregunta errónea,
  así que no hay fallback.

Si todos los tramos `typesafe` fallan de forma reintentable, los tramos restantes de la ruta
responden a la petición como un **chat completion normal**: el contenido es la respuesta del
modelo de chat, no un mapa `answers`. Si añades tramos de chat, tu cliente debe manejar ambas
formas; el campo `model` de la respuesta te dice cuál has recibido. Si la ruta no tiene otros
tramos, la petición falla con `502` y el código de error `all_legs_failed`.

Una petición puede llevar un bloque `vertex` junto con el bloque `jev`. No tiene efecto sobre
Jev, pero si la petición hace fallback y el bloque `vertex` tiene funcionalidades nativas, el
carril Vertex nativo la sirve desde los tramos `vertex` de la ruta. Consulta
[Funcionalidades nativas de Vertex](native-vertex.md).

## Extracción híbrida {#hybrid-extraction}

La extracción híbrida evalúa candidatos con Jev y después extrae datos estructurados solo de
los candidatos que superan la evaluación. Añade una especificación `extract` al bloque `jev`:

```json
{
  "model": "verify-orgs",
  "messages": [{ "role": "user", "content": "Find the official website of Acme Ltd." }],
  "jev": {
    "questions": {
      "c0_match": { "type": "noul", "instructions": "Candidate 0 is Acme Ltd's own website." },
      "c1_match": { "type": "noul", "instructions": "Candidate 1 is Acme Ltd's own website." }
    },
    "extract": {
      "floor": 0.7,
      "candidates": [
        { "key": "c0", "question": "c0_match", "text": "<page excerpt 0>" },
        { "key": "c1", "question": "c1_match", "text": "<page excerpt 1>" }
      ],
      "prompt": "Extract the organisation's contact details from candidate {{key}}:\n\n{{text}}",
      "response_schema": {
        "type": "object",
        "properties": { "email": { "type": "string" }, "phone": { "type": "string" } }
      }
    }
  }
}
```

| Campo | Descripción |
|---|---|
| `candidates` | Cada candidato tiene una `key`, el `text` del que extraer y la `question` que lo filtra, que debe ser una pregunta `noul` de `questions`. |
| `floor` | Entre 0 (excluido) y 1. Un candidato sobrevive cuando la respuesta `noul` de su pregunta es al menos el umbral. |
| `prompt` | Instrucciones de extracción. Debe contener `{{text}}`, que se sustituye por el texto del candidato; `{{key}}` se sustituye por su clave. |
| `response_schema` | Un objeto de esquema JSON que debe cumplir la extracción. |

Synapse hace las preguntas a Jev y después ejecuta una extracción por cada candidato
superviviente en los tramos de la ruta que no son `typesafe`, con el prompt como mensaje de
sistema. El esquema se envía como `response_format` en el carril estándar, o como
`vertex.response_schema` en el carril Vertex nativo cuando la petición tiene funcionalidades
nativas de Vertex. La ruta necesita al menos un tramo de chat.

La respuesta añade un bloque `jev` junto a los campos habituales:

```json
{
  "object": "chat.completion",
  "model": "jev-latest",
  "jev": {
    "answers": { "c0_match": { "...": "..." }, "c1_match": { "...": "..." } },
    "survivors": ["c0"],
    "degraded": false
  },
  "choices": [{
    "index": 0,
    "message": { "role": "assistant", "content": "{\"c0\":{\"email\":\"...\",\"phone\":\"...\"}}" },
    "finish_reason": "stop"
  }],
  "usage": { "prompt_tokens": 812, "completion_tokens": 64, "total_tokens": 876 }
}
```

- `content` es una cadena JSON que asocia la clave de cada superviviente con su extracción. No
  aparece cuando ningún candidato alcanza el umbral.
- `usage` suma los tokens de Jev y los de cada extracción.
- `degraded` es `true` cuando todos los tramos `typesafe` fallaron de forma reintentable, en
  cuyo caso Synapse extrae de **todos** los candidatos y `answers` es `{}`, o cuando falló la
  extracción de un superviviente, en cuyo caso esa clave falta en `content`. La respuesta
  mantiene la misma forma en ambos casos.

Synapse devuelve `400` cuando la petición tiene `stream: true`, la ruta no tiene tramos de
chat, la petición tiene funcionalidades nativas de Vertex pero la ruta no tiene ningún tramo
`vertex`, `floor` está fuera de rango, `prompt` no contiene `{{text}}`, `response_schema` no es
un objeto, o la `question` de un candidato falta o no es una pregunta `noul`.

## Streaming {#streaming}

Con `stream: true`, una decisión de Jev llega como un único `chat.completion.chunk` con todo
el contenido JSON, seguido del fragmento de finalización y de `data: [DONE]`. Si la petición
hace fallback a tramos de chat, se transmite como cualquier chat completion. La extracción
híbrida no admite streaming.

## Registro de costes y métricas {#ledger-and-metrics}

Cada respuesta de Jev escribe una fila en el registro con el proveedor `typesafe` y el carril
`jev`. La extracción híbrida añade una fila por cada extracción correcta, y todas comparten el
`request_id` de la petición. `synapse_jev_extraction_total`, etiquetada por `route` y
`degraded`, cuenta las respuestas híbridas. Consulta [Registro de costes](cost-ledger.md).

## Passthrough de Jev {#jev-passthrough}

Para llamar a Jev sin el formato de chat completions, envía su cuerpo nativo `{state,
questions}` a `POST /typesafe/v1/systemone`. Synapse reenvía el cuerpo literalmente con su
propia `TYPESAFE_API_KEY`, establece `model` a `jev-latest` si lo omites, y devuelve el estado
y el cuerpo de TypeSafe sin cambios. No hay fallback ni streaming. El uso se anota en el
registro con el carril `passthrough`, atribuido con los mismos encabezados que las peticiones
de chat. Sin `TYPESAFE_API_KEY`, el endpoint devuelve `400`. Las peticiones se escanean con la
política `default`, y ningún ajuste asigna otra política a este endpoint.

## Carril Jev y router Jev {#jev-lane-and-jev-router}

El carril Jev responde a tus preguntas. El [router Jev](jev-router.md) es una funcionalidad
distinta que usa Jev para elegir un nivel de modelo para peticiones de chat ordinarias. Un
bloque `jev` enviado a una ruta `strategy = "jev"` devuelve `400`.
