---
sidebar_position: 1
title: Funcionalidades nativas de Vertex
description: Usa la caché de contexto de Vertex AI, medios de Cloud Storage, esquemas de respuesta estrictos, la configuración de razonamiento y la elección nativa de herramientas a través de la API compatible con OpenAI de Synapse.
---

Usa el carril Vertex nativo cuando una petición necesite algo que solo Vertex AI puede hacer:
reutilizar una caché de contexto, leer un vídeo directamente de Cloud Storage, forzar la
respuesta a un esquema JSON con decodificación restringida o fijar la configuración de
razonamiento de Gemini. Mantienes el formato de petición de OpenAI y añades un bloque
`vertex`; Synapse envía la petición directamente a la API REST de Vertex en lugar de pasar por
el adaptador genérico compatible con OpenAI, que descartaría esos campos. Si una petición no
necesita nada de esto, omite el bloque `vertex` y viajará por el carril estándar, con fallback
a cualquier proveedor.

## Cómo llega una petición al carril nativo {#how-a-request-reaches-the-native-lane}

Una petición va al carril Vertex nativo cuando su bloque `vertex` tiene `cached_content`,
`response_schema` o `thinking_config`, o una entrada de `media_uris` que empieza por `gs://`.
Un bloque `vertex` sin ninguno de estos, por ejemplo solo con URIs de medios `https://`, va al
carril estándar, que ignora el bloque.
[Detección de carril](../overview/architecture.md#lane-detection) tiene el orden completo,
incluido cómo un bloque `jev` tiene prioridad.

En el carril nativo:

- Solo participan los tramos `vertex` de la ruta. Los tramos OpenAI, Qwen y `oai_compat` no
  pueden expresar estas funcionalidades, así que se omiten en lugar de recibir una petición a
  la que se le han quitado las funcionalidades en silencio.
- Una ruta sin ningún tramo `vertex` devuelve `400` con el código de error
  `native_feature_unsupported`.
- Synapse siempre llama a `:streamGenerateContent`, y consolida el stream en una única
  respuesta para los clientes sin streaming.
- Cada tramo llama a Vertex AI en su `region`, o en `VERTEX_LOCATION` si no tiene. Consulta
  [Regiones](../configuration/routes.md#regions).

Una ruta `strategy = "jev"` sirve las peticiones nativas desde el nivel más cercano que tenga
un tramo `vertex`; consulta [Router Jev](jev-router.md#tier-fallback).

## Caché de contexto {#context-caching}

`cached_content` nombra un recurso `cachedContents` de Vertex. Synapse lo pasa como
`cachedContent`, de modo que Gemini lee el contenido en caché en lugar de que tú lo vuelvas a
enviar:

```json
{
  "model": "gemini-flash",
  "messages": [{ "role": "user", "content": "Which animals appear in the video?" }],
  "vertex": {
    "cached_content": "projects/my-gcp-project/locations/us/cachedContents/1234567890123456789"
  }
}
```

Synapse no crea ni gestiona cachés; créalas con la API de Vertex AI. Una caché está ligada a
un modelo y a una ubicación, así que debe coincidir con el tramo que sirve la petición. Si la
caché ha caducado o pertenece a otro modelo o ubicación, Vertex AI rechaza la petición y
Synapse devuelve `400` con el mensaje de Vertex. Con varios tramos `vertex` en modelos
distintos, solo puede tener éxito el tramo que coincide con la caché; los demás fallan con
`400` y detienen la cadena, así que da a las peticiones con caché una ruta cuyo primer tramo
`vertex` coincida con la caché.

El [tutorial de caché](../get-started/tutorials/native-vertex-caching.md) explica paso a paso
cómo crear una caché y consultarla.

:::warning
El registro de costes valora los tokens de entrada en caché al precio `input` completo,
mientras que Vertex AI los factura con descuento. Consulta
[Registro de costes](cost-ledger.md#accuracy).
:::

## Medios en Cloud Storage {#cloud-storage-media}

`media_uris` enumera objetos de Cloud Storage para que Gemini los lea. Synapse adjunta cada
URI al último mensaje de usuario como una parte de archivo con tipo MIME `video/mp4`:

```json
{
  "model": "gemini-flash",
  "messages": [{ "role": "user", "content": "Describe this video." }],
  "vertex": { "media_uris": ["gs://cloud-samples-data/video/animals.mp4"] }
}
```

Como cada URI se etiqueta como `video/mp4`, usa `media_uris` para vídeo. Envía las imágenes
inline, como partes de contenido `image_url` con una URL `data:` en base64, que los carriles
estándar y Vertex nativo admiten.

## Salida estructurada {#structured-output}

`response_schema` es un esquema JSON que restringe la salida de Gemini. Synapse lo envía como
`generationConfig.responseSchema` con `responseMimeType` igual a `application/json`, de
modo que Vertex AI usa decodificación restringida y el contenido del mensaje es una cadena
JSON que cumple el esquema:

```json
{
  "model": "gemini-flash",
  "messages": [{ "role": "user", "content": "List three primary colours." }],
  "vertex": {
    "response_schema": {
      "type": "object",
      "properties": { "colours": { "type": "array", "items": { "type": "string" } } },
      "required": ["colours"]
    }
  }
}
```

Synapse elimina del contenido las partes de razonamiento de Gemini, así que la cadena es solo
el documento JSON. Vertex AI acepta un subconjunto de JSON Schema; consulta la documentación
de salida estructurada de Google para ver qué admite `responseSchema`.

El campo `response_format` de OpenAI no se usa en el carril nativo. En el carril estándar,
`response_format` con `json_schema` o `json_object` se pasa en cambio al proveedor a través
del crate `genai`.

## Configuración de razonamiento {#thinking-configuration}

`thinking_config` se copia literalmente en `generationConfig.thinkingConfig`, así que puedes
usar lo que acepte el modelo, por ejemplo `{ "thinkingLevel": "low" }` para Gemini 3 o
`{ "thinkingBudget": 2048 }` para Gemini 2.5.

El carril nativo ignora el campo `reasoning_effort` de OpenAI. En una ruta
`strategy = "jev"`, el `effort` de un nivel se convierte en un presupuesto de razonamiento
(`thinkingBudget`) en los tramos nativos, salvo que la petición lleve su propio
`thinking_config`, que siempre gana. Consulta [Esfuerzo](jev-router.md#effort).

## Llamadas a herramientas {#tool-calling}

Las `tools` en formato OpenAI se convierten en `functionDeclarations` de Vertex, y
`tool_choice` se respeta mediante `toolConfig.functionCallingConfig`:

| `tool_choice` | `mode` de Vertex |
|---|---|
| `"auto"` | `AUTO` |
| `"none"` | `NONE` |
| `"required"` | `ANY` |
| `{ "type": "function", "function": { "name": "..." } }` | `ANY` |

Nombrar una función establece el modo `ANY`, que obliga al modelo a llamar a una función pero
no lo restringe a la que has nombrado.

Las llamadas a herramientas de la respuesta reciben los ids `call_0`, `call_1`, etc. Cuando
devuelves el resultado de una herramienta, Synapse usa el `name` del mensaje `role: "tool"`
como nombre de función de Vertex, así que asigna a `name` el nombre de la función, además de
`tool_call_id`. Consulta
[Streaming y llamadas a herramientas](streaming-and-tools.md#tool-calling) para el ciclo
completo.

## Otros campos de la petición {#other-request-fields}

En el carril nativo, Synapse también reenvía `temperature` y `max_tokens` (como
`maxOutputTokens`). Envía los mensajes `system` como turnos de usuario, no como un
`systemInstruction` de Vertex. Otros campos de OpenAI, como `top_p` o `stop`, no se reenvían.

## Tiempos de espera {#timeouts}

El carril nativo no tiene tiempo de espera de primer fragmento ni de inactividad. Cada llamada
está limitada solo por el tiempo de espera HTTP del proveedor, `SYNAPSE_REQUEST_TIMEOUT_SECS`
(por defecto 120 segundos), que cubre la respuesta completa, así que una generación que dure
más falla aunque siga produciendo salida. Aumenta el valor si esperas respuestas largas.
Consulta [Streaming y llamadas a herramientas](streaming-and-tools.md#timeouts).

## Fallback entre tramos Vertex {#fallback-between-vertex-legs}

Cuando la apertura del stream falla con una respuesta `5xx`, `429` o `408`, un error de
conexión o un tiempo de espera agotado, Synapse prueba el siguiente tramo `vertex` de la ruta.
Cualquier otro `4xx` detiene la cadena y devuelve `400`. Una vez que Vertex AI ha aceptado la
petición, un fallo posterior no se reintenta, ni siquiera para clientes sin streaming.
Consulta [Cadenas de fallback](fallback-chains.md#native-vertex-lane).

## Clientes del SDK de Gemini {#gemini-sdk-clients}

Los clientes que usan los SDK de Gemini de Google en lugar de un SDK de OpenAI pueden llamar a
los endpoints passthrough nativos de Gemini de Synapse,
`POST /v1beta/models/<model>:<action>`, `POST /v1/models/<model>:<action>` y
`POST /google/models/<model>:<action>`. Synapse reenvía el cuerpo literalmente a Vertex AI con
sus propias credenciales, contabiliza en el registro las llamadas `generateContent` y
`streamGenerateContent`, y ante un error de conexión o una respuesta `5xx`, `429` o `408`
prueba los siguientes tramos `vertex` de la ruta que incluye ese modelo. Las peticiones
`generateContent` y `streamGenerateContent` se escanean con la política de la ruta
coincidente, si no con `default`; consulta
[Qué se escanea](../configuration/guardrails-policy.md#what-is-scanned). `<model>` puede ser
el alias de una ruta, que selecciona sus tramos y su política. Un modelo que no aparece en
ninguna ruta recibe `404`, salvo con `SYNAPSE_PASSTHROUGH_MODELS=any`.
Consulta [Passthrough de Gemini](../reference/http-api.md#gemini-passthrough) para el
comportamiento completo, incluidos los tiempos de espera.
