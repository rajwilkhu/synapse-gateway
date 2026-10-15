---
sidebar_position: 2
title: Inicio rápido
description: Ejecuta el gateway Synapse con una configuración de dos rutas y envíale peticiones estándar, en streaming, con fallback y de Vertex AI nativo.
---

En este inicio rápido escribes una configuración con dos rutas, arrancas el gateway y le
envías una petición estándar, una petición en streaming, una petición que puede hacer
fallback a OpenAI y una petición de Vertex AI nativo. Después consultas lo que ha costado
cada petición.

## Requisitos previos {#prerequisites}

- Docker, o Synapse instalado con Cargo (consulta [Instalación](installation.md)).
- Un proyecto de Google Cloud con la API de Vertex AI habilitada y credenciales que puedan
  llamarla: una clave de cuenta de servicio con el rol Vertex AI User para Docker, o
  `gcloud auth application-default login` para un binario local.
- Opcionalmente, una clave de API de OpenAI para el segundo tramo de la ruta `chat`. Si no la
  tienes, consulta el consejo de [Arrancar el gateway](#start-the-gateway).
- La herramienta de línea de comandos `sqlite3` en tu máquina, para el último paso,
  [Ver lo que ha costado](#see-what-it-cost).

El gateway arranca sin credenciales válidas: al iniciarse solo comprueba que
`VERTEX_PROJECT_ID` y `OPENAI_API_KEY` estén definidas. Las credenciales se usan en la
primera petición. Si faltan o no son válidas, una petición estándar falla con `502` y el
código de error `all_legs_failed`. Una petición Vertex nativa falla con `502` y
`upstream_error` cuando faltan las credenciales, o con `400` cuando Vertex AI las rechaza.

## Crear la configuración {#create-the-configuration}

Crea un directorio de trabajo con una carpeta `config/` para la configuración y una
carpeta `data/` para el registro de costes:

```bash
mkdir -p synapse-quickstart/config synapse-quickstart/data && cd synapse-quickstart
```

Guarda las rutas como `config/routes.toml`:

```toml title="config/routes.toml"
[routes."gemini-flash"]
legs = [
  { provider = "vertex", model = "gemini-3.5-flash-lite", region = "us" },
]

[routes."chat"]
legs = [
  { provider = "vertex", model = "gemini-3.5-flash-lite", region = "us" },
  { provider = "openai", model = "gpt-4o-mini" },
]
```

Cada tabla `[routes."<alias>"]` define un nombre de modelo que tus clientes pueden enviar.
`gemini-flash` siempre usa Gemini 3.5 Flash-Lite en Vertex AI. `chat` prueba primero Gemini
y hace fallback a `gpt-4o-mini` de OpenAI si falla el tramo de Vertex. `region` fija el
tramo de Vertex a la multirregión `us` de Vertex en el carril Vertex nativo.

Guarda los precios como `config/pricing.toml`:

```toml title="config/pricing.toml"
"vertex:gemini-3.5-flash-lite" = { input = 0.30, output = 2.50 }
"openai:gpt-4o-mini" = { input = 0.15, output = 0.60 }
```

Los precios están en USD por 1,000,000 de tokens, indexados por `provider:model`. Synapse
los usa para valorar cada petición en el registro de costes; un modelo sin precio cuesta 0.

Guarda los guardrails como `config/guardrails.toml`:

```toml title="config/guardrails.toml"
[guardrails.default]
scanners = ["prompt_injection", "secrets"]
```

Una ruta sin `policy` se escanea con `default`, así que ambas rutas bloquean la inyección de
prompts y los secretos filtrados. El gateway no arranca sin este archivo; consulta
[Política de guardrails](../configuration/guardrails-policy.md).

## Arrancar el gateway {#start-the-gateway}

El gateway se ejecuta en primer plano. Déjalo funcionando y envía las peticiones del resto
de este inicio rápido desde una segunda terminal, en el mismo directorio
`synapse-quickstart`.

### Docker {#docker}

Coloca tu clave de cuenta de servicio en el directorio de trabajo como `sa.json` y
ejecuta:

```bash
docker run --rm -p 8080:8080 -p 9090:9090 \
  -e VERTEX_PROJECT_ID=my-gcp-project \
  -e OPENAI_API_KEY=sk-... \
  -e GOOGLE_APPLICATION_CREDENTIALS=/secrets/sa.json \
  -e SYNAPSE_LEDGER_SQLITE_DSN="sqlite:///app/data/synapse.db?mode=rwc" \
  -v "$(pwd)/sa.json:/secrets/sa.json:ro" \
  -v "$(pwd)/config:/app/config" \
  -v "$(pwd)/data:/app/data" \
  sustentabilitas/synapse-gateway
```

En un Mac con Apple silicon, añade `--platform linux/amd64` después de `docker run`: la
imagen solo se construye para `linux/amd64`.

La imagen se ejecuta como el usuario no root `synapse` (UID 1001) sin directorio home, así
que no puede ver tus credenciales de `gcloud`; la clave montada ocupa su lugar.
`SYNAPSE_LEDGER_SQLITE_DSN` coloca el registro de SQLite en la carpeta `data/` montada,
para que sobreviva al contenedor y puedas consultarlo desde tu máquina.

:::note
En Linux, el UID 1001 debe poder leer `sa.json` y escribir en `data/`. En una máquina de
desarrollo, la solución más sencilla es:

```bash
chmod a+r sa.json && chmod a+rwx data
```

Si el gateway no puede abrir el registro, anota el error en el log y sigue sirviendo
peticiones sin registrarlas.
:::

Montar `config/` sustituye todo el directorio `/app/config` de la imagen, así que solo se
usan los archivos que aportes.

### Cargo {#cargo}

Desde el directorio `synapse-quickstart`, con el binario de
`cargo install synapse-gateway`:

```bash
gcloud auth application-default login
export VERTEX_PROJECT_ID=my-gcp-project
export OPENAI_API_KEY=sk-...
SYNAPSE_ROUTES_PATH=config/routes.toml SYNAPSE_PRICING_PATH=config/pricing.toml \
  SYNAPSE_GUARDRAILS_PATH=config/guardrails.toml \
  SYNAPSE_LEDGER_SQLITE_DSN="sqlite://data/synapse.db?mode=rwc" \
  synapse-gateway
```

Para ejecutarlo desde un clon del repositorio, sustituye `synapse-gateway` por
`cargo run --release -p synapse-gateway --manifest-path <path-to-clone>/Cargo.toml`. Las
rutas de archivo siguen siendo relativas a tu directorio de trabajo.

En ambos casos, la API escucha en el puerto `8080`, las métricas de Prometheus en el puerto
`9090`, y el registro de costes se escribe en `data/synapse.db`.

:::tip
¿No tienes clave de API de OpenAI? Define `SYNAPSE_PROVIDER_VALIDATION=lenient` en lugar de
`OPENAI_API_KEY`. Synapse descarta entonces el tramo `openai` y arranca con `chat` servida
solo por Vertex. El valor por defecto, `strict`, se niega a arrancar cuando una ruta hace
referencia a un proveedor sin credenciales.
:::

## Comprobar que está en marcha {#check-that-it-is-running}

```bash
curl -s http://localhost:8080/health
```

La respuesta es el cuerpo de texto plano `ok`.

Lista los alias de modelo que pueden usar tus clientes:

```bash
curl -s http://localhost:8080/v1/models
```

```json
{"object":"list","data":[{"id":"chat","object":"model","owned_by":"synapse"},{"id":"gemini-flash","object":"model","owned_by":"synapse"}]}
```

## Enviar una petición estándar {#send-a-standard-request}

```bash
curl -s http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "x-synapse-tenant: my-team" \
  -d '{
    "model": "gemini-flash",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

La respuesta es un objeto `chat.completion` estándar de OpenAI. El encabezado
`x-synapse-tenant` atribuye los tokens y el coste de la petición a `my-team` en el
registro; sin él, las peticiones se atribuyen a `unattributed`.

Como la petición es OpenAI puro, cualquier SDK de OpenAI también funciona: apunta su URL
base a `http://localhost:8080/v1` y usa `gemini-flash` como modelo. Synapse no tiene
autenticación de entrada, pero la mayoría de los SDKs exigen una clave de API, así que pasa
cualquier cadena no vacía. Consulta
[Seguridad](../operating/security.md#callers-arent-authenticated) y
[Limitaciones y hoja de ruta](../reference/limitations-roadmap.md).

## Enviar una petición en streaming {#send-a-streaming-request}

```bash
curl -sN http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "x-synapse-tenant: my-team" \
  -d '{
    "model": "gemini-flash",
    "messages": [{"role": "user", "content": "Count to 5."}],
    "stream": true
  }'
```

La respuesta es un stream de eventos enviados por el servidor (SSE) en formato OpenAI:
líneas `data: {...}` que llevan objetos `chat.completion.chunk` y terminan con
`data: [DONE]`.

## Enviar una petición con fallback {#send-a-request-with-fallback}

La ruta `chat` tiene dos tramos, así que este paso necesita `OPENAI_API_KEY`. Envíale el
mismo tipo de petición:

```bash
curl -s http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "x-synapse-tenant: my-team" \
  -d '{
    "model": "chat",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

El campo `model` de la respuesta indica el tramo que ha respondido. Mientras Vertex AI
funcione correctamente, es `gemini-3.5-flash-lite`. Para ver el fallback, reinicia el
gateway con `VERTEX_PROJECT_ID` apuntando a un proyecto que no existe y repite ambas
peticiones: `gemini-flash` falla ahora con `502` y el código de error `all_legs_failed`,
mientras que `chat` responde con `"model": "gpt-4o-mini"`. Tu cliente recibe una respuesta
correcta en ambos casos.

## Enviar una petición Vertex nativa {#send-a-native-vertex-request}

Añade un bloque `vertex` para usar funcionalidades exclusivas de Vertex. Esta petición
adjunta un vídeo de ejemplo público de Cloud Storage:

```bash
curl -s http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "x-synapse-tenant: my-team" \
  -d '{
    "model": "gemini-flash",
    "messages": [{"role": "user", "content": "Describe this video."}],
    "vertex": {
      "media_uris": ["gs://cloud-samples-data/video/animals.mp4"]
    }
  }'
```

La respuesta es un `chat.completion` normal que describe el vídeo. El bloque `vertex` envía
la petición al carril Vertex nativo, que conserva los campos exclusivos de Vertex que el
carril estándar descartaría. Solo la sirven los tramos `vertex`: en la ruta `chat`, el
tramo de OpenAI se omite. Una ruta sin ningún tramo `vertex` devuelve `400` con el código
de error `native_feature_unsupported`. Consulta
[Carril Vertex nativo](../overview/architecture.md#native-vertex-lane) para ver cómo
funciona.

### Opcional: reutilizar una caché de contexto {#optional-reuse-a-context-cache}

Si has creado una caché de contexto de Vertex AI, añade su nombre de recurso como
`cached_content`:

```json
"vertex": {
  "media_uris": ["gs://cloud-samples-data/video/animals.mp4"],
  "cached_content": "projects/my-gcp-project/locations/us/cachedContents/abc123"
}
```

La caché debe existir en tu proyecto para el mismo modelo y la misma región que el tramo
de Vertex de la ruta, aquí `gemini-3.5-flash-lite` en `us`. Sin esa caché, Vertex AI
rechaza la petición.

## Ver lo que ha costado {#see-what-it-cost}

Con el registro de SQLite por defecto, el gateway escribe una fila por cada petición
completada. Consulta el archivo del registro desde tu máquina:

```bash
sqlite3 data/synapse.db \
  'SELECT tenant, route, provider, model, lane, input_tokens, output_tokens, cost_usd FROM usage_events;'
```

Cada petición que has enviado aparece como una fila atribuida a `my-team`, con su carril
(`standard` o `native`) y su coste según `pricing.toml`.

## Próximos pasos {#next-steps}

- Sigue los tutoriales:
  [guardar un vídeo en caché y obtener respuestas estructuradas](tutorials/native-vertex-caching.md),
  [enrutar peticiones por dificultad con Jev](tutorials/jev-tiers.md) y
  [hacer fallback entre proveedores](tutorials/fallback-across-providers.md).
- Añade tus propias rutas, tramos y niveles con la
  [referencia de rutas](../configuration/routes.md), y define los precios con
  [Precios](../configuration/pricing.md).
- Aprende cada funcionalidad con las guías: el
  [carril Vertex nativo](../guides/native-vertex.md), el
  [router Jev](../guides/jev-router.md), las [cadenas de fallback](../guides/fallback-chains.md),
  el [streaming y las llamadas a herramientas](../guides/streaming-and-tools.md), la
  [atribución por tenant](../guides/tenant-attribution.md) y el
  [registro de costes](../guides/cost-ledger.md).
- Despliégalo con [Docker](../deployment/docker.md) o
  [Docker Compose](../deployment/docker-compose.md) y después repasa la
  [lista de comprobación para producción](../deployment/production-checklist.md).
- Supervísalo con [Métricas](../operating/metrics.md) y el
  [panel de Grafana](../operating/grafana-dashboard.md).
- Consulta los endpoints y los códigos de error en la
  [referencia de la API HTTP](../reference/http-api.md), los ajustes en
  [Variables de entorno](../configuration/environment-variables.md) y lo que Synapse todavía
  no hace en [Limitaciones y hoja de ruta](../reference/limitations-roadmap.md).
- Lee [Arquitectura](../overview/architecture.md) para ver cómo encajan los carriles y las
  cadenas de fallback.
- Consulta rutas, tramos, niveles y tenants en [Conceptos](../overview/concepts.md).
