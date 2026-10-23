---
sidebar_position: 4
title: Limitaciones y hoja de ruta
description: Lo que el gateway y sus crates hermanos aún no hacen, y las carencias que debes tener en cuenta al planificar.
---

Esta página reúne lo que Synapse no hace, para que puedas planificar en consecuencia antes de
depender de ello. Cada punto enlaza a la página que lo trata en detalle.

## Planificado {#planned}

Esto no está en la versión actual y está planificado para versiones futuras:

- **Autenticación de entrada.** No hay claves de API ni autenticación de clientes. Ejecuta el
  gateway detrás de un API gateway, un ingress o un service mesh que autentique a los clientes;
  consulta [Seguridad](../operating/security.md#callers-arent-authenticated).
- **Limitación de tasa.** Nada limita la rapidez con la que un cliente puede gastar tu cuota.
  Aplica los límites delante del gateway.
- **Recarga dinámica de rutas.** No hay ninguna API de administración para cambiar las rutas.
  Cada archivo de configuración se lee una sola vez al arrancar, así que un cambio requiere un
  reinicio; consulta [Claves de configuración](./configuration-keys.md).

## Campos de la petición {#request-fields}

El gateway acepta cualquier campo de chat de OpenAI, pero solo reenvía algunos y descarta el
resto sin error:

- En el **carril estándar**, se descartan `max_tokens`, `tool_choice`, `top_p`, `stop`, `seed` y
  cualquier otro campo que no figure en
  [`POST /v1/chat/completions`](./http-api.md#post-v1chatcompletions). Por tanto, una respuesta
  puede ser más larga que el `max_tokens` que enviaste. Consulta
  [Llamadas a herramientas](../guides/streaming-and-tools.md#tool-calling) para `tool_choice`.
- En el **carril Vertex nativo**, solo se reenvían `temperature`, `max_tokens`, `tools`,
  `tool_choice` y el bloque `vertex`. `response_format` se ignora (usa
  `vertex.response_schema`), y los mensajes `system` se envían como turnos de usuario. Consulta
  [Otros campos de la petición](../guides/native-vertex.md#other-request-fields).
- Los fragmentos del streaming no llevan `usage`. Los recuentos de tokens van al registro de
  costes y a las métricas.
- Los cuerpos de las peticiones están limitados a 2 MB. Envía los medios grandes a Vertex AI
  mediante una URI de Cloud Storage.

## Resiliencia {#resilience}

- **Los reintentos y los circuit breakers solo se aplican al carril estándar.** Cada tramo
  `(provider, model)` del carril estándar tiene su propia política de reintentos y su propio
  circuit breaker, y registra las métricas `synapse_resilience_*` y los logs
  `synapse::resilience`. En los carriles Vertex nativo, Jev y de embeddings, cada tramo tiene
  un intento por petición sin circuit breaker, y el siguiente tramo de la cadena proporciona
  la conmutación por error. Consulta
  [Cadenas de fallback](../guides/fallback-chains.md#the-order-of-legs).

## Tiempos de espera {#timeouts}

- **El carril Vertex nativo no tiene tiempo de espera de primer fragmento ni de inactividad.**
  Solo `SYNAPSE_REQUEST_TIMEOUT_SECS` acota una llamada, y cubre toda la respuesta, así que un
  primer token lento espera el tiempo de espera completo antes de que se pruebe el siguiente
  tramo `vertex`. Consulta [Tiempos de espera](../guides/native-vertex.md#timeouts).
- **El streaming en el carril estándar no tiene tiempo de espera de inactividad** una vez que ha
  llegado el primer fragmento.
- **`SYNAPSE_REQUEST_TIMEOUT_SECS` cubre toda la respuesta** en todos los carriles, así que las
  generaciones largas fallan a mitad del stream salvo que lo aumentes. Consulta
  [Streaming y llamadas a herramientas](../guides/streaming-and-tools.md#timeouts).

## Observabilidad {#observability}

- **Sin trazas.** El gateway solo registra métricas de OpenTelemetry; no emite spans de
  OpenTelemetry ni propaga el contexto de traza a los proveedores. Consulta
  [Logs](../operating/logging.md#tracing).
- **Sin métrica para los chat completions fallidos.** Las métricas de peticiones solo cuentan
  las peticiones que produjeron una respuesta. Mide las tasas de error delante del gateway;
  consulta
  [Qué cuentan las métricas de peticiones](../operating/metrics.md#what-the-request-metrics-count).
- **Solo logs en texto plano.** No hay formato de log JSON ni log de accesos. Consulta
  [Logs](../operating/logging.md).
- **Peculiaridades de exposición de `synapse-proxy`.** Los contadores del proxy tienen un sufijo
  `_total` duplicado y su histograma de duración tiene buckets dimensionados para milisegundos.
  Consulta [Métricas de synapse-proxy](./metrics-catalogue.md#synapse-proxy).

## Precisión del registro de costes {#cost-ledger-accuracy}

Los costes del registro son estimaciones calculadas a partir de los recuentos de tokens de los
proveedores y de tu `pricing.toml`. La entrada en caché de Vertex nativo se valora a la tarifa
completa, los tokens de razonamiento de Vertex nativo no se cuentan, los intentos fallidos y los
streams abandonados no registran tokens, los modelos de chat sin precio cuestan 0, y se pueden
descartar filas cuando la cola está llena. [Precisión](../guides/cost-ledger.md#accuracy)
explica cada carencia.

## Operación {#operations}

- **Sin apagado ordenado.** El gateway no espera a que terminen las peticiones al recibir
  `SIGTERM`: las peticiones en curso se cortan y las filas del registro de costes que siguen en
  la cola se pierden.
  Consulta [Detención](../deployment/docker.md#stopping).
- **El registro A2A está en memoria, por instancia.** Los agentes registrados mediante la API de
  administración solo existen en la instancia que recibió la llamada, y se pierden al reiniciar.
  Carga los agentes compartidos desde `a2a.toml`; consulta
  [Ejecutar más de una instancia](../deployment/production-checklist.md#running-more-than-one-instance).
- **SQLite es de un solo nodo.** Varias instancias necesitan un destino compartido, como
  Postgres; consulta [Destinos](../guides/cost-ledger.md#sinks).
- **Las imágenes de Docker son solo `linux/amd64`.** En hosts ARM, ejecútalas con emulación
  mediante `--platform linux/amd64`; consulta [Docker](../deployment/docker.md).

## synapse-mcp {#synapse-mcp}

El crate del gateway MCP enruta cada servidor MCP por separado. Todavía no se admite:

- **Agregación de herramientas entre servidores.** No hay una superficie de herramientas
  combinada; a cada servidor se accede en su propio path `/mcp/<server>`.
- **Compatibilidad con SSE** para servidores upstream que solo hablan el transporte SSE heredado.
- **Varias vinculaciones de identidad simultáneas.** Solo hay un overlay de identidad activo a la
  vez, en línea con el `ContextStore` de `synapse-context`.
