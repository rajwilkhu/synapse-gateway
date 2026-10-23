---
sidebar_position: 5
title: Cadenas de fallback
description: Cómo recorre Synapse los tramos de una ruta cuando falla un proveedor, qué fallos pasan al siguiente tramo en cada carril y cómo diseñar cadenas que conmuten bien.
---

Usa una cadena de fallback siempre que la caída de un proveedor o de una región, un límite de
tasa o un tiempo de espera agotado no deba convertirse en un error para tus usuarios. Una ruta
enumera varios tramos y Synapse los prueba en orden hasta que uno responde; el cliente envía
una petición y recibe una respuesta, sin saber que un tramo ha fallado. Las cadenas también te
permiten poner primero un modelo más barato o más rápido y reservar uno más potente.

Las claves se describen en [Rutas](../configuration/routes.md), y el
[tutorial de fallback](../get-started/tutorials/fallback-across-providers.md) muestra una
cadena que conmuta de Vertex AI a OpenAI.

## El orden de los tramos {#the-order-of-legs}

- En una ruta estática, la cadena son los `legs` de la ruta, en orden.
- En una ruta `strategy = "jev"`, la cadena empieza por los tramos del nivel elegido, sigue con
  los de cada nivel más difícil y después con los de cada nivel más fácil. Consulta
  [Router Jev](jev-router.md#tier-fallback).
- Con la validación de proveedores permisiva, los tramos cuyo proveedor no se puede construir
  se eliminan al arrancar, y la cadena es lo que queda. Consulta
  [Proveedores](../configuration/providers.md#strict-and-lenient-validation).

Cada petición la sirve un solo tramo. Synapse nunca envía la misma petición a dos tramos a la
vez. En el **carril estándar**, cada tramo `(provider, model)` tiene su propio circuit breaker
y su política de reintentos (hasta 3 intentos en total con backoff exponencial ante errores
transitorios antes de conmutar al siguiente tramo, y omitiendo los tramos cuyo circuit breaker
esté abierto). En los carriles Vertex nativo, Jev y de embeddings, cada tramo tiene un intento
por petición. Consulta
[Limitaciones y hoja de ruta](../reference/limitations-roadmap.md#resilience).

## Qué pasa al siguiente tramo {#what-moves-to-the-next-leg}

Lo que cuenta como fallo depende del carril que sirve la petición y, en el carril estándar,
de si el cliente usa streaming.

### Carril estándar {#standard-lane}

En una petición **sin streaming**, cualquier fallo pasa al siguiente tramo:

- una respuesta de error del proveedor, incluidos errores `4xx` como un modelo desconocido o
  una clave de API no válida;
- que no llegue el primer fragmento dentro de `SYNAPSE_REQUEST_TIMEOUT_SECS`;
- un intervalo entre fragmentos mayor que `SYNAPSE_STREAM_IDLE_TIMEOUT_SECS`;
- un stream que se corta o termina antes de que el modelo acabe.

Synapse almacena en búfer la respuesta completa de cada tramo antes de enviar nada, así que
incluso un tramo que falla a mitad de camino se sustituye limpiamente por el siguiente.

En una petición **con streaming**, esos mismos fallos pasan al siguiente tramo solo hasta que
llega el primer fragmento. A partir de ahí Synapse se compromete con ese tramo y reenvía los
fragmentos según llegan; un fallo posterior termina el stream con un evento `error`. Consulta
[Streaming y llamadas a herramientas](streaming-and-tools.md#fallback-while-streaming).

### Carril Vertex nativo {#native-vertex-lane}

Las peticiones con [funcionalidades nativas de Vertex](native-vertex.md) solo usan los tramos
`vertex` de la ruta, y son más estrictas con lo que reintentan:

- Una respuesta `5xx`, `429` o `408`, un error de conexión o un tiempo de espera agotado al
  abrir el stream pasa al siguiente tramo `vertex`.
- Cualquier otro `4xx` detiene la cadena y devuelve `400` con el mensaje de Vertex. Estos
  errores, como un esquema no válido o una caché caducada, fallarían en todos los tramos.
- Una vez que Vertex AI acepta la petición, Synapse queda fijado a ese tramo, tanto
  para clientes con streaming como sin él. Un fallo a partir de ese momento hace fallar la
  petición con `502` y el código de error `upstream_error`.

### Carril Jev {#jev-lane}

Las peticiones con un bloque `jev` prueban primero los tramos `typesafe` de la ruta. Un `429`,
`408`, `5xx` o un error de conexión pasa al siguiente; cualquier otro error detiene la
petición con `400`. Cuando todos los tramos `typesafe` fallan de forma reintentable, los demás
tramos de la ruta responden como un chat completion normal. Consulta
[Carril Jev](jev-lane.md#when-jev-fails).

### Embeddings {#embeddings}

Los alias de embeddings prueban sus tramos en orden. Cualquier error pasa al siguiente tramo,
y se omite un tramo cuyo proveedor no esté configurado. Consulta
[Embeddings](embeddings.md#fallback).

## Cuando fallan todos los tramos {#when-every-leg-fails}

| Carril | Respuesta |
|---|---|
| Estándar | `502` con el código de error `all_legs_failed` y un array `failures` que indica el proveedor, el modelo y el mensaje de error de cada tramo, o `503` con `all_circuits_open` cuando el circuit breaker de todos los tramos está abierto. |
| Vertex nativo | `502` con el código de error `upstream_error` y el error del último tramo. |
| Jev, sin otros tramos | `502` con el código de error `all_legs_failed`. |

```json
{
  "error": {
    "type": "all_legs_failed",
    "code": "all_legs_failed",
    "message": "all legs of route 'chat' failed",
    "failures": [
      { "provider": "vertex", "model": "gemini-3.5-flash-lite", "message": "..." },
      { "provider": "openai", "model": "gpt-4o-mini", "message": "..." }
    ]
  }
}
```

## Qué se registra {#what-gets-recorded}

Solo se registra el tramo que sirvió la petición: la fila del registro, el campo `model` de la
respuesta y la etiqueta `model` de `synapse_requests_total` lo nombran a él. Los intentos
fallidos no se registran, así que los tokens que un proveedor consumió en un tramo que se
cortó a mitad de camino no aparecen en el registro. Consulta
[Registro de costes](cost-ledger.md#accuracy).

## Diseñar una cadena {#design-a-chain}

- **Combina proveedores o regiones.** Dos tramos en el mismo proveedor y la misma región
  suelen fallar a la vez. Pon detrás del primer tramo un proveedor distinto, o el mismo modelo
  en otra región.
- **Pon primero el tramo por el que quieres pagar.** Los tramos posteriores solo se ejecutan
  cuando fallan los anteriores, así que ordénalos por preferencia, no por potencia.
- **Ten en cuenta las peticiones nativas.** Una petición Vertex nativa solo puede usar tramos
  `vertex`, así que una ruta que las sirva necesita más de un tramo `vertex` para poder
  conmutar.
- **Prueba cada tramo por separado.** En el carril estándar, un tramo mal configurado, como un
  nombre de modelo mal escrito, conmuta en silencio en cada petición, añadiendo latencia y
  ocultando el problema. Después de cambiar modelos o credenciales, llama a cada tramo a
  través de una ruta de un solo tramo.
- **Dimensiona los tiempos de espera para la cadena.** Cada tramo puede consumir hasta
  `SYNAPSE_REQUEST_TIMEOUT_SECS` antes de que empiece el siguiente, así que una cadena de tres
  tramos puede tardar tres veces más en fallar.
