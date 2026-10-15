---
sidebar_position: 1
title: Variables de entorno
description: Todas las variables de entorno que lee el gateway Synapse, con su valor por defecto y lo que controlan.
---

El binario del gateway Synapse se configura con variables de entorno, además de los archivos
TOML cuyas rutas definen algunas de ellas. El gateway las lee una sola vez al arrancar, así que
reinícialo después de cualquier cambio.

## Servidor {#server}

| Variable | Valor por defecto | Descripción |
|---|---|---|
| `SYNAPSE_ADDR` | `0.0.0.0:8080` | Dirección y puerto de la API: todos los endpoints HTTP excepto las métricas. |
| `SYNAPSE_ADMIN_ADDR` | `127.0.0.1:8081` | Dirección y puerto de los endpoints de administración de A2A, `/internal/a2a/*`. `shared` los sirve en `SYNAPSE_ADDR`. Los endpoints de administración no tienen autenticación: mantén esta dirección privada. |
| `SYNAPSE_METRICS_ADDR` | `0.0.0.0:9090` | Dirección y puerto del endpoint de Prometheus, `GET /metrics`. Debe ser una dirección IP y un puerto; un nombre de host hace fallar el arranque. |

## Archivos de configuración {#configuration-files}

Las rutas relativas se resuelven contra el directorio de trabajo del gateway. En la imagen de
Docker es `/app`, así que los valores por defecto apuntan a los archivos de `/app/config`.

| Variable | Valor por defecto | Descripción |
|---|---|---|
| `SYNAPSE_ROUTES_PATH` | `config/routes.toml` | Alias de ruta, sus tramos y niveles. Obligatorio: el gateway no arranca sin él. Consulta [Rutas](routes.md). |
| `SYNAPSE_PRICING_PATH` | `config/pricing.toml` | Precios por `provider:model`, usados para calcular el coste de las filas del registro de costes. Obligatorio. Consulta [Precios](pricing.md). |
| `SYNAPSE_GUARDRAILS_PATH` | `config/guardrails.toml` | Políticas de guardrails. Obligatorio salvo con `SYNAPSE_GUARDRAILS=optional`. Consulta [Política de guardrails](guardrails-policy.md). |
| `SYNAPSE_AI_TASK_TYPES_PATH` | `config/ai_task_types.toml` | Asigna los alias de ruta al tipo de tarea de IA que se registra en las filas del registro de costes. Opcional: sin el archivo, todas las peticiones registran `simple`. Consulta [Tipos de tarea de IA](../guides/tenant-attribution.md#ai-task-types). |
| `SYNAPSE_A2A_PATH` | `config/a2a.toml` | Archivo semilla del registro de agentes A2A en memoria que se sirve en el puerto de la API (rutas públicas) y en `SYNAPSE_ADMIN_ADDR` (rutas de administración). Opcional: sin el archivo, el registro empieza vacío. |

## Proveedores {#providers}

| Variable | Valor por defecto | Descripción |
|---|---|---|
| `SYNAPSE_PROVIDER_VALIDATION` | `strict` | Qué hacer cuando una ruta hace referencia a un proveedor que este proceso no puede construir, porque su credencial no está definida o el id es desconocido. `strict` se niega a arrancar. `lenient` descarta esos tramos, elimina las rutas que se quedan sin tramos y arranca. Cualquier valor distinto de `lenient` equivale a `strict`. |
| `SYNAPSE_GUARDRAILS` | `required` | `required` se niega a arrancar cuando falta el archivo de guardrails, o cuando una ruta o un alias de embeddings nombra una política que el archivo no define. `optional` arranca con los guardrails desactivados cuando falta el archivo, y registra un aviso por cada política no definida. Cualquier valor distinto de `optional` equivale a `required`. |
| `SYNAPSE_PASSTHROUGH_MODELS` | `routed` | `routed` hace que el passthrough de Gemini reenvíe solo un alias de ruta o un modelo que sea un tramo `vertex` de una ruta; cualquier otro recibe `404`. `any` reenvía los modelos desconocidos como un único tramo. Cualquier valor distinto de `any` equivale a `routed`. |

Cada proveedor lee sus propias variables de credencial y de endpoint. La tabla las enumera;
consulta [Proveedores](providers.md) para saber cuáles son obligatorias y cómo las trata la
validación.

| Variable | Valor por defecto | Usada por |
|---|---|---|
| `VERTEX_PROJECT_ID` | — | `vertex`: el proyecto de Google Cloud al que se llama. |
| `VERTEX_PROJECT` | — | `vertex`: nombre heredado, que solo se lee cuando `VERTEX_PROJECT_ID` no está definida. |
| `VERTEX_LOCATION` | `global` | `vertex`: ubicación para los tramos Vertex nativos sin `region` y para los embeddings de Vertex. |
| `GOOGLE_APPLICATION_CREDENTIALS` | — | `vertex`: ruta a una clave de cuenta de servicio, leída por Application Default Credentials. |
| `OPENAI_API_KEY` | — | `openai`: clave de API. |
| `OPENAI_BASE_URL` | `https://api.openai.com/v1` | `openai`: URL base de la API. |
| `DASHSCOPE_API_KEY` | — | `qwen`: clave de API de DashScope. |
| `DASHSCOPE_BASE_URL` | `https://dashscope-intl.aliyuncs.com/compatible-mode/v1` | `qwen`: URL base de la API. |
| `OAI_COMPAT_BASE_URL` | — | `oai_compat`: URL base de tu servidor compatible con OpenAI. |
| `OAI_COMPAT_API_KEY` | — | `oai_compat`: clave de API, si tu servidor la necesita. |
| `TYPESAFE_API_KEY` | — | Tramos `typesafe`, rutas `strategy = "jev"` y el passthrough `/typesafe/v1/systemone`. |
| `TYPESAFE_BASE_URL` | `https://api.typesafe.ai` | URL base de la API de TypeSafe, para despliegues autoalojados y pruebas. |

## Streaming y tiempos de espera {#streaming-and-timeouts}

| Variable | Valor por defecto | Descripción |
|---|---|---|
| `SYNAPSE_REQUEST_TIMEOUT_SECS` | `120` | Tiempo máximo, en segundos, para que un tramo produzca su primer fragmento en el carril estándar. Un tramo que lo supera se abandona y se prueba el siguiente. El mismo valor es el tiempo de espera HTTP de cada llamada a un proveedor, y cubre la respuesta completa. |
| `SYNAPSE_STREAM_IDLE_TIMEOUT_SECS` | `60` | Intervalo máximo, en segundos, entre dos fragmentos para las peticiones sin streaming en el carril estándar. Un tramo que se detiene durante ese tiempo falla y se prueba el siguiente. Las peticiones con streaming no tienen tiempo de espera de inactividad una vez que llega su primer fragmento. Consulta [Tiempos de espera](../guides/streaming-and-tools.md#timeouts). |

Ambos valores deben ser números enteros; cualquier otra cosa detiene el gateway al arrancar.
El carril Vertex nativo no tiene tiempo de espera de primer fragmento ni de inactividad: solo
lo limita el tiempo de espera HTTP del proveedor.

## Registro de costes {#ledger}

| Variable | Valor por defecto | Descripción |
|---|---|---|
| `SYNAPSE_LEDGER_BACKENDS` | `sqlite` | Destinos del registro de costes separados por comas: `sqlite`, `postgres`, `pubsub`, `sns`. Cada evento de uso va a todos los destinos. Un nombre desconocido o repetido detiene el gateway al arrancar. |
| `SYNAPSE_LEDGER_BACKEND` | — | Un único destino, que solo se lee cuando `SYNAPSE_LEDGER_BACKENDS` no está definida. |
| `SYNAPSE_LEDGER_SQLITE_DSN` | `sqlite://synapse.db?mode=rwc` | Base de datos SQLite. Recurre a `SYNAPSE_LEDGER_DSN` y, si no, al valor por defecto: un archivo `synapse.db` en el directorio de trabajo. |
| `SYNAPSE_LEDGER_POSTGRES_DSN` | — | Cadena de conexión de Postgres. Recurre a `SYNAPSE_LEDGER_DSN`. Obligatoria para el destino `postgres`. |
| `SYNAPSE_LEDGER_DSN` | — | DSN heredado compartido por los destinos SQLite y Postgres. Es preferible usar las variables de cada destino. |
| `SYNAPSE_LEDGER_PUBSUB_TOPIC` | — | ID del topic de Google Cloud Pub/Sub. Obligatoria para el destino `pubsub`. |
| `SYNAPSE_LEDGER_PUBSUB_PROJECT` | — | Proyecto del topic de Pub/Sub. Recurre a `VERTEX_PROJECT_ID` y, si no, a `VERTEX_PROJECT`. |
| `SYNAPSE_LEDGER_SNS_TOPIC_ARN` | — | ARN del topic de AWS SNS. Obligatoria para el destino `sns`. |
| `SYNAPSE_LEDGER_SNS_REGION` | — | Región de AWS del topic. Sin ella, se aplica la cadena de región por defecto del SDK de AWS. |
| `SYNAPSE_EMBED_DEFAULT_INPUT_PRICE_PER_MTOK` | `0.10` | USD por 1,000,000 de tokens de entrada para los modelos de embeddings sin entrada en `pricing.toml`. Un valor que no se puede interpretar equivale a `0.10`. |

Los destinos `postgres`, `pubsub` y `sns` necesitan las features de Cargo `ledger-postgres`,
`ledger-pubsub` y `ledger-sns`; la imagen de Docker las incluye todas. Un destino cuya feature
falta, o que no puede conectar al arrancar, se registra en el log y se omite. Si no conecta
ningún destino, el gateway arranca igualmente y sirve peticiones sin registrar el uso. Consulta
la [guía del registro de costes](../guides/cost-ledger.md) para saber qué recibe cada destino y
cómo de fiable es la entrega.

## Tenants {#tenancy}

| Variable | Valor por defecto | Descripción |
|---|---|---|
| `SYNAPSE_DEFAULT_TENANT` | `unattributed` | Tenant que se registra para las peticiones sin encabezado `x-synapse-tenant`. |

Consulta [Atribución por tenant](../guides/tenant-attribution.md) para ver los encabezados de
atribución.

## Telemetría {#telemetry}

| Variable | Valor por defecto | Descripción |
|---|---|---|
| `OTEL_EXPORTER_OTLP_ENDPOINT` | — | URL base de un colector de OpenTelemetry, como `http://otel-collector:4318`. Si está definida, las métricas también se envían por OTLP/HTTP a `<endpoint>/v1/metrics`. `OTEL_EXPORTER_OTLP_METRICS_ENDPOINT` no se lee. |
| `OTEL_SERVICE_NAME` | `synapse-gateway` | Atributo de recurso `service.name` en las métricas exportadas. |
| `OTEL_METRIC_EXPORT_INTERVAL` | `60000` | Intervalo de envío OTLP en milisegundos, leído por el SDK de OpenTelemetry. |
| `RUST_LOG` | `info` | Filtro de logs, por ejemplo `info` o `synapse=debug`. |

El scraping de Prometheus en `SYNAPSE_METRICS_ADDR` funciona tanto si OTLP está configurado
como si no.
