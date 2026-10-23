---
sidebar_position: 1
title: Pipeline de una petición
description: Cómo avanza un chat completion por el código fuente del gateway, paso a paso, desde el manejador HTTP hasta la escritura en el registro de costes.
---

Esta página sigue una petición `POST /v1/chat/completions` por el código fuente de
`synapse-gateway`, en el orden en que lo ejecuta el código, con un enlace al archivo de cada
paso. Léela cuando quieras modificar el gateway, depurar una respuesta sorprendente o comprobar
qué paso produjo un error. Para saber qué significa cada paso para un cliente, consulta
[Arquitectura](../overview/architecture.md).

Todas las rutas de archivo están bajo
[`crates/synapse-gateway/src`](https://github.com/sustentabilitas/synapse-gateway/tree/main/crates/synapse-gateway/src).

```text
server.rs            analiza el JSON, lee los encabezados x-synapse-*, elige un id de petición
  │
gateway.rs           chat_routed (con búfer) o chat_stream (streaming)
  │
route_planner.rs     busca el alias → routing_strategy → guardrails → tramos o decisión de Jev
  │
gateway.rs           comprobaciones del bloque jev y de la especificación extract, rama híbrida
  │
routing/classify.rs  carril estándar, Vertex nativo o Jev
  │
  ├── routing/executor.rs   carril estándar: recorre los tramos a través de genai
  ├── vertex_native.rs      carril Vertex nativo: recorre los tramos vertex
  └── jev_native.rs         carril Jev: recorre los tramos typesafe y luego recurre a los de chat
  │
server.rs            genera JSON o server-sent events, añade los encabezados de enrutamiento
  │
gateway.rs           record(), o StreamSideEffects al hacer drop
  ├── ledger/        encola la fila de uso; una tarea en segundo plano la escribe en cada destino
  └── telemetry.rs   cuenta la petición y sus tokens
```

## 1. Manejador HTTP {#1-http-handler}

[`server.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/server.rs)
construye el router de axum y gestiona `POST /v1/chat/completions` en `chat_completions`.

- El extractor `Json` de axum analiza el cuerpo y lo convierte en un `ChatRequest`, definido en
  [`routing/request.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/routing/request.rs).
  Un cuerpo que no se puede analizar se rechaza aquí, con el error en texto plano de axum en
  lugar del error JSON del gateway. Los campos que el struct no nombra se recogen en
  `passthrough`.
- `request_ctx` lee los siete encabezados `x-synapse-*` y los guarda en un `RequestCtx`.
- El id de la petición es el encabezado `x-synapse-message` cuando está presente y no vacío; si
  no, un UUID nuevo. Se convierte en el `id` de la respuesta y en el `request_id` del registro de
  costes.
- `stream: true` va a `Gateway::chat_stream`; cualquier otra cosa, a `Gateway::chat_routed`.

## 2. Entrada al gateway {#2-gateway-entry}

[`gateway.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/gateway.rs)
contiene `Gateway`, el gateway en proceso al que llaman tanto la capa HTTP como las
[aplicaciones que lo embeben](../guides/embedding-as-library.md). `chat_routed` y `chat_stream`
ejecutan los mismos pasos hasta la ejecución; se diferencian en cómo ejecutan los tramos y en
cuándo registran el uso. Ambos inician aquí un temporizador, así que la métrica de latencia
incluye la planificación y la decisión de Jev.

## 3. Planificación de la ruta {#3-route-planning}

[`route_planner.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/route_planner.rs)
convierte la petición en un `RoutePlan`, una lista ordenada de tramos, en `plan_route`:

1. **Busca el alias** en la `RouteTable` de
   [`routing/table.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/routing/table.rs).
   Un alias desconocido falla con `404 model_not_found`.
2. **Resuelve `routing_strategy`** con `resolve_mode` en
   [`routing/jev_router.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/routing/jev_router.rs).
   Un valor no válido falla con `400`.
3. **Ejecuta los guardrails** mediante `Gateway::guard_input`, que elige la `policy` de la ruta o
   `default` y llama a
   [`guard/engine.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/guard/engine.rs).
   Las políticas vienen de
   [`guard/policy.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/guard/policy.rs),
   los escáneres de
   [`guard/scanners.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/guard/scanners.rs),
   el enmascaramiento `[REDACTED:<scanner>]` in situ de
   [`guard/mask.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/guard/mask.rs),
   y la clasificación semántica `noul` con Jev (que se ejecuta sobre el texto enmascarado cuando
   los escáneres de tramos no bloquearon) de
   [`guard/classifier.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/guard/classifier.rs).
   Los escaneos distintos de `pass` emiten un evento estructurado en `synapse::guard`; una
   comprobación respondida del clasificador o un bloqueo de entrada escribe una fila en el
   registro de costes con `lane = "guard"` (`guard_classifier` o `guard_input`), y un bloqueo
   falla con `400 content_blocked` antes de cualquier llamada a un proveedor o al router Jev.
4. **Elige los tramos.** Una ruta estática usa sus tramos en orden. Una ruta
   `strategy = "jev"` pasa por `plan_tiered`:
   - una petición con un bloque `jev` falla con `400`;
   - en una petición Vertex nativa, solo se conservan los tramos `vertex`, y se omiten los
     niveles que se quedan sin tramos;
   - `decide` llama a Jev con el `timeout_ms` de la ruta, u omite la llamada con
     `"routing_strategy": "static"`. Una decisión interpretada escribe su propia fila en el
     registro de costes;
   - `jev_router.rs` selecciona el nivel y el esfuerzo y ordena los tramos: el nivel elegido,
     cada nivel más difícil y después cada nivel más fácil;
   - la decisión se cuenta en `synapse_routing_decisions_total` y se registra en el log como
     `route planned` con el target `synapse::routing`.

Un problema con Jev nunca hace fallar la planificación; se recurre a `default_tier`. Consulta
[Router Jev](../guides/jev-router.md#failure-behaviour).

## 4. Comprobaciones de la petición {#4-request-checks}

De vuelta en `gateway.rs`:

- `require_jev_block` falla con `400` cuando la cadena tiene tramos `typesafe` y la petición no
  tiene preguntas `jev`.
- `validate_extract` en
  [`routing/jev_extract.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/routing/jev_extract.rs)
  comprueba una especificación de extracción híbrida.
- Una petición con búfer que tiene `jev.extract` sale aquí del pipeline hacia `chat_hybrid` en
  [`jev_hybrid.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/jev_hybrid.rs),
  que pregunta a Jev, ejecuta después una extracción por cada candidato superviviente en los
  tramos de chat y registra una fila en el registro de costes por cada una.

## 5. Clasificación del carril {#5-lane-classification}

`classify` en
[`routing/classify.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/routing/classify.rs)
elige el carril solo a partir del cuerpo: un bloque `jev` con preguntas significa el carril Jev;
un bloque `vertex` con `cached_content`, `response_schema`, `thinking_config` o una URI de medios
`gs://` significa el carril Vertex nativo; cualquier otra cosa, el carril estándar. Consulta
[Detección de carril](../overview/architecture.md#lane-detection).

## 6. Ejecución {#6-execution}

Cada carril recorre los tramos del plan de forma distinta.

**Carril estándar.**
[`routing/executor.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/routing/executor.rs)
llama a los proveedores a través del crate `genai`, con el catálogo de proveedores construido en
[`providers/mod.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/providers/mod.rs)
y
[`providers/genai_provider.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/providers/genai_provider.rs),
con reintentos y circuit breakers por tramo definidos en
[`resilience.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/resilience.rs).
`to_genai_options` construye las opciones de cada petición, que es donde los campos se reenvían o
se descartan.

- `execute_buffered_with_timeouts` almacena en búfer todo el stream de cada tramo, con los
  tiempos de espera de primer fragmento y de inactividad, y pasa al siguiente tramo ante
  cualquier fallo.
- `execute_streaming_with_timeouts` abre cada tramo y espera su primer fragmento. El primer tramo
  que produce uno queda fijado; los fallos posteriores llegan al cliente como un evento de
  error.
- Ambos devuelven `502 all_legs_failed` con el fallo de cada tramo cuando se agota la cadena, o
  `503 all_circuits_open` cuando el circuit breaker de todos los tramos está abierto.

**Carril Vertex nativo.** `Gateway::native_committed` recorre los tramos `vertex` de la cadena,
convierte el esfuerzo de cada tramo en un presupuesto de razonamiento con `with_leg_thinking` y
llama a `stream_generate` en
[`vertex_native.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/vertex_native.rs),
que traduce la petición al formato de Vertex y llama a la región del tramo. Un `5xx`, un `429`,
un `408` o un error de conexión pasa al siguiente tramo; cualquier otro `4xx` detiene la cadena.
En una petición con búfer, `collect_committed` vacía el stream fijado en una única
respuesta completa. Las credenciales de Google vienen de
[`providers/vertex_auth.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/providers/vertex_auth.rs).

**Carril Jev.** `Gateway::jev_attempt` envía las preguntas a cada tramo `typesafe` a través de
[`jev_native.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/jev_native.rs).
Cuando todos los tramos fallan con un error reintentable, los tramos restantes responden como un
chat completion en el carril Vertex nativo o en el estándar.

El tiempo de espera HTTP de los proveedores, `SYNAPSE_REQUEST_TIMEOUT_SECS`, se fija en el
cliente de cada proveedor cuando se construye el gateway en
[`main.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/main.rs).

## 7. Respuesta {#7-response}

`server.rs` genera el resultado, después de que los `output_scanners` opcionales de
[`guard/output.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/guard/output.rs)
revisen las respuestas con búfer o retengan `output_holdback_chars` en los streams:

- `openai_json` construye un `chat.completion` mediante el `Accumulator` de
  [`routing/stream.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/routing/stream.rs),
  y `hybrid_json`, el sobre híbrido.
- `sse_body` genera un stream como eventos `chat.completion.chunk` con `stream_item_to_sse_json`,
  convierte un error a mitad del stream en un evento de error y añade `[DONE]` al final.
- `with_routing_headers` añade los encabezados `x-synapse-*` a partir del `RoutingReport` del
  plan, definido en `routing/jev_router.rs`, y `with_guard_header` añade `x-synapse-guard`
  cuando la entrada o la respuesta con búfer se enmascaró o bloqueó.
- Un `GatewayError` de cualquier paso se convierte en el cuerpo de error JSON y el estado en
  [`error.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/error.rs).

## 8. Contabilidad del uso {#8-usage-accounting}

El uso se registra para las peticiones que produjeron una respuesta, así como para los bloqueos
de guardrails de entrada y las comprobaciones del clasificador Jev (`lane = "guard"`).

- **Con búfer:** `Gateway::record` en `gateway.rs` calcula el precio del completado con
  [`pricing.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/pricing.rs),
  encola una fila en el registro de costes y emite las métricas de la petición mediante
  `GenAiSpan::emit_metrics` en
  [`observability.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/observability.rs),
  antes de devolver la respuesta.
- **Streaming:** el `GuardedStream` que devuelve `chat_stream` envuelve el stream fijado en
  un guard `StreamSideEffects`. El guard lee los recuentos de tokens del último elemento del
  stream y hace el mismo trabajo en su `Drop`, así que se ejecuta una sola vez termine como
  termine el stream: completado, error o desconexión del cliente. Un stream que falló a mitad se
  registra con el estado `error`.

La fila va a `LedgerHandle::enqueue` en
[`ledger/mod.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/ledger/mod.rs),
que nunca espera: pone la fila en una cola acotada de 10,000, o la descarta y cuenta
`synapse_ledger_dropped_total`. Una tarea en segundo plano escribe cada fila en el destino, o en
todos los destinos mediante `FanoutLedger`.
[`ledger/connect.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/ledger/connect.rs)
conecta los destinos al arrancar, y cada destino tiene su propio archivo: `sqlite.rs`,
`postgres.rs`, `pubsub.rs` y `sns.rs`. El formato de fila está en
[`ledger/event.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/ledger/event.rs).

Las métricas son los instrumentos de
[`telemetry.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/telemetry.rs),
que también construye los exportadores de Prometheus y OTLP. Consulta el
[catálogo de métricas](../reference/metrics-catalogue.md).

## Otros endpoints {#other-endpoints}

Los demás endpoints siguen caminos más cortos por los mismos componentes:

- **Embeddings.** `Gateway::embed_contextual` en `gateway.rs` busca el alias en
  [`routing/embeddings.rs`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/src/routing/embeddings.rs),
  escanea los elementos de `input` bajo la política de guardrails del alias (`Gateway::guard_lane`
  con `GuardLane::Embeddings`), recorre sus tramos con los embedders de
  [`embeddings/`](https://github.com/sustentabilitas/synapse-gateway/tree/main/crates/synapse-gateway/src/embeddings)
  y registra el uso.
- **Passthrough de Gemini.** `gemini_passthrough` en `server.rs` escanea las acciones con
  contenido mediante `screen_gemini` (`GuardLane::Passthrough`), reenvía el cuerpo con
  `VertexNativeProvider::passthrough_request`, elige los tramos de fallback con
  `RouteTable::vertex_fallback_chain` y mide el uso con un guard de `Drop` como el del streaming.
- **Passthrough de Jev.** `jev_passthrough` en `server.rs` escanea las cadenas de `state` y
  `questions` bajo la política `default` (`GuardLane::Jev`), reenvía el cuerpo a través de
  `jev_native.rs` y lo mide de la misma forma.
- **A2A.** Los endpoints del registro vienen del crate `synapse-a2a`, integrados en el router en
  `main.rs`. Consulta [Crates del workspace](./workspace-crates.md#synapse-a2a).
