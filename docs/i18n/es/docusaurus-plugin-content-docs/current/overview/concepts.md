---
sidebar_position: 3
title: Conceptos
description: Los términos que se usan en toda la documentación de Synapse, desde rutas y tramos hasta carriles, niveles y el registro de costes.
---

Esta página define los términos que usa el resto de la documentación. Cada entrada es
breve; sigue los enlaces para ver los detalles.

## Ruta {#route}

Una entrada con nombre en `routes.toml`, como `[routes."chat"]`. Los clientes eligen una
ruta enviando su nombre, el **alias**, en el campo `model` de una petición de chat
completion, y `GET /v1/models` lista todos los alias. Un alias desconocido devuelve `404`
con el código de error `model_not_found`. Consulta
[Flujo de una petición](architecture.md#request-flow) y
[Rutas](../configuration/routes.md).

## Tramo {#leg}

Un proveedor y un modelo dentro de una ruta, por ejemplo
`{ provider = "vertex", model = "gemini-3.5-flash-lite", region = "us" }`. El proveedor es
uno de `vertex`, `openai`, `qwen`, `oai_compat` o `typesafe`. El campo opcional `region`
fija el tramo a una ubicación de Vertex en el carril Vertex nativo. Consulta
[Claves de los tramos](../configuration/routes.md#leg-keys),
[Regiones](../configuration/routes.md#regions) y
[Proveedores](../configuration/providers.md).

## Carril {#lane}

La ruta de backend que sirve una petición: **estándar** (proveedores compatibles con OpenAI
a través del crate `genai`), **Vertex nativo** (la API REST de Vertex) o **Jev** (TypeSafe
System One). El cuerpo de la petición determina el carril. Consulta
[Carriles](architecture.md#lanes) y [Detección de carril](architecture.md#lane-detection), y
las guías de [Vertex nativo](../guides/native-vertex.md) y del
[carril Jev](../guides/jev-lane.md).

## Estrategia {#strategy}

Cómo elige una ruta sus tramos, definido con la clave `strategy` de la ruta. `static`, el
valor por defecto, usa los `legs` de la ruta en orden. `jev` declara niveles en lugar de
tramos, y el router Jev elige un nivel para cada petición. Un cliente puede enviar
`"routing_strategy": "static"` para omitir la decisión en una petición concreta a una ruta
`jev`. Consulta [Rutas](../configuration/routes.md) y
[Anular la decisión por petición](../guides/jev-router.md#override-the-decision-per-request).

## Nivel {#tier}

Un nivel de dificultad de una ruta `strategy = "jev"`, declarado como
`[[routes."<alias>".tiers]]` con un `name`, una `description` del trabajo para el que sirve,
un `effort` de razonamiento y sus propios `legs`. Una ruta tiene de 2 a 10 niveles,
ordenados del más fácil al más difícil. La clave `default_tier` de la tabla
`[routes."<alias>".jev_router]` de la ruta indica el nivel que sirve cuando Jev no puede
decidir. Consulta la [guía del router Jev](../guides/jev-router.md) y
[Claves de los niveles](../configuration/routes.md#tier-keys).

## Cadena de fallback {#fallback-chain}

Los tramos ordenados que Synapse prueba para una petición hasta que uno tiene éxito. En una
ruta estática son los `legs` de la ruta; en una ruta `jev` empieza en el nivel elegido y
continúa por los niveles más difíciles y después por los más fáciles. Consulta
[Flujo de una petición](architecture.md#request-flow) y la
[guía de cadenas de fallback](../guides/fallback-chains.md).

## Tenant {#tenant}

El cliente, equipo o aplicación al que se atribuye una petición, tomado del encabezado
`x-synapse-tenant`. Sin ese encabezado, Synapse usa `SYNAPSE_DEFAULT_TENANT` (por defecto,
`unattributed`). El tenant se guarda en las filas del registro, pero nunca es una etiqueta
de métrica, porque su valor lo controlan los clientes. Consulta
[Atribución por tenant](../guides/tenant-attribution.md) y
[Los encabezados de tenant se aceptan sin comprobar](../operating/security.md#tenant-headers-are-trusted).

## Tipo de tarea de IA {#ai-task-type}

Una etiqueta en cada fila del registro que describe el tipo de trabajo que ha realizado el
gateway. Synapse usa el encabezado `x-synapse-ai-task-type` si está presente; si no, el tipo
de tarea asociado al alias de la ruta en `config/ai_task_types.toml`; y, si no, `simple`.
Consulta [Tipos de tarea de IA](../guides/tenant-attribution.md#ai-task-types) y
[`ai_task_types.toml`](../reference/configuration-keys.md#ai_task_typestoml).

## Registro de costes {#cost-ledger}

El registro del uso de tokens y del coste de cada petición completada, escrito de forma
asíncrona para que nunca ralentice la respuesta. El coste sale de `pricing.toml` (USD por
1,000,000 de tokens, indexado por `provider:model`); los modelos que no aparecen cuestan 0.
Las filas van a SQLite por defecto, o a Postgres, y también pueden publicarse en Google
Cloud Pub/Sub y AWS SNS. Consulta la [guía del registro de costes](../guides/cost-ledger.md)
y [Precios](../configuration/pricing.md).

## Política de guardrails {#guardrail-policy}

Una lista con nombre de escáneres de entrada, como la detección de inyección de prompts,
secretos o PII, definida bajo `[guardrails.<name>]` en `guardrails.toml`. Una política o bien
bloquea las peticiones que coinciden con `400` (modo `block`, el valor por defecto) o bien solo
las registra (modo `observe`). Una ruta se adhiere con `policy = "<name>"`; las rutas sin
política usan la política `default` y, si no hay política `default`, esas rutas no se escanean.
Sin `guardrails.toml` el gateway se niega a arrancar, salvo con `SYNAPSE_GUARDRAILS=optional`,
que desactiva los guardrails. Consulta [Política de
guardrails](../configuration/guardrails-policy.md) y [Respuesta de
bloqueo](../configuration/guardrails-policy.md#block-response).
