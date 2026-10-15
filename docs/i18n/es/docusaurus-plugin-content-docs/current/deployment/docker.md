---
sidebar_position: 1
title: Docker
description: Ejecuta las imágenes del gateway y del proxy de Synapse con tu propia configuración, tus credenciales y tu backend de registro de costes.
---

Puedes ejecutar Synapse en cualquier runtime de contenedores a partir de las imágenes de Docker
Hub. Esta página explica qué contienen las imágenes y cómo proporcionar la configuración, las
credenciales y un backend de registro de costes. Para probar el gateway por primera vez, sigue
el [inicio rápido](../get-started/quickstart.md); para ejecutarlo junto a Postgres y
Prometheus, consulta [Docker Compose](docker-compose.md).

## Imágenes {#images}

| Imagen | Binario | Puertos | Se ejecuta como |
|---|---|---|---|
| `sustentabilitas/synapse-gateway` | `synapse-gateway` | `8080` API, `9090` métricas de Prometheus, `8081` administración de A2A (loopback del contenedor) | `synapse` (UID 1001) |
| `sustentabilitas/synapse-proxy` | `synapse-proxy` | `8787` tráfico del proxy, `9090` métricas de Prometheus | `synapse` (UID 1001) |

Cada versión se etiqueta con su número de versión (por ejemplo
`sustentabilitas/synapse-gateway:2.0.0`) y como `latest`. Cada push a `main` se publica como
`edge`. Las imágenes se compilan solo para `linux/amd64`. En una máquina ARM, como un Mac con
Apple silicon, añade `--platform linux/amd64` a `docker pull` y `docker run` (o
`platform: linux/amd64` en Compose) y Docker las ejecuta con emulación; sin ello, el pull falla
con `no matching manifest for linux/arm64/v8`.

Ambas imágenes se construyen a partir de `debian:bookworm-slim`. El usuario `synapse` no tiene
directorio personal ni shell de inicio de sesión, y el directorio de trabajo es `/app`.

## Ejecutar el gateway {#run-the-gateway}

Un contenedor típico del gateway monta un directorio de configuración y una clave de cuenta de
servicio de Google Cloud, y lee las claves de los proveedores de un archivo de entorno:

```bash
docker run -d --name synapse \
  -p 8080:8080 -p 9090:9090 \
  --env-file synapse.env \
  -e GOOGLE_APPLICATION_CREDENTIALS=/secrets/sa.json \
  -v "$(pwd)/sa.json:/secrets/sa.json:ro" \
  -v "$(pwd)/config:/app/config:ro" \
  sustentabilitas/synapse-gateway:2.0.0
```

con un `synapse.env` que contenga, por ejemplo:

```bash
VERTEX_PROJECT_ID=my-gcp-project
OPENAI_API_KEY=sk-...
SYNAPSE_DEFAULT_TENANT=my-deployment
```

Comprueba que ha arrancado:

```bash
curl -s http://localhost:8080/health
```

`GET /health` devuelve el cuerpo en texto plano `ok` en cuanto el listener de la API está
activo. Úsalo para los health checks del contenedor y las sondas del balanceador de carga. No
llama a ningún proveedor.

### Configuración {#configuration}

El gateway lee sus archivos TOML de `/app/config`:

| Archivo | Obligatorio | Referencia |
|---|---|---|
| `routes.toml` | Sí | [Rutas](../configuration/routes.md) |
| `pricing.toml` | Sí | [Precios](../configuration/pricing.md) |
| `guardrails.toml` | No | [Política de guardrails](../configuration/guardrails-policy.md) |
| `ai_task_types.toml` | No | [Tipos de tarea de IA](../guides/tenant-attribution.md#ai-task-types) |
| `a2a.toml` | No | [Claves de configuración](../reference/configuration-keys.md#a2atoml) (semilla del registro de agentes A2A) |

La imagen incluye la configuración de ejemplo de
[`crates/synapse-gateway/config/`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/config),
que hace referencia a proveedores para los que quizá no tengas credenciales. Monta siempre tu
propio directorio sobre `/app/config`. El montaje sustituye el directorio entero, así que solo
se usan los archivos que proporcionas: si no incluyes `guardrails.toml`, el gateway se niega a
arrancar, salvo con `SYNAPSE_GUARDRAILS=optional`, que desactiva los guardrails.

Para guardar los archivos en otro lugar del contenedor, define `SYNAPSE_ROUTES_PATH`,
`SYNAPSE_PRICING_PATH` y las demás variables de ruta que se enumeran en
[Variables de entorno](../configuration/environment-variables.md#configuration-files).

El UID 1001 debe poder leer los archivos de configuración y la clave de cuenta de servicio. El
gateway nunca escribe en su directorio de configuración, así que un montaje de solo lectura
(`:ro`) funciona.

### Credenciales {#credentials}

Cada proveedor lee sus credenciales de variables de entorno; consulta
[Proveedores](../configuration/providers.md). Pasa los secretos con `--env-file` o con el
almacén de secretos de tu orquestador, no en la línea de comandos, donde acaban en el historial
de tu shell y en `docker inspect`.

Vertex AI usa Google Application Default Credentials. Como el usuario del contenedor no tiene
directorio personal, las credenciales de `gcloud auth application-default login` de tu máquina
no son visibles dentro del contenedor. Puedes:

- montar una clave de cuenta de servicio y apuntar `GOOGLE_APPLICATION_CREDENTIALS` a su ruta,
  como arriba; o
- ejecutar en Google Cloud (Cloud Run, GKE con Workload Identity, Compute Engine), donde el
  servidor de metadatos proporciona las credenciales y no necesitas ninguna de las dos cosas.

La cuenta necesita permiso para llamar a Vertex AI, por ejemplo el rol Vertex AI User.

Con la validación estricta de proveedores por defecto, el gateway se niega a arrancar cuando
una ruta usa un proveedor cuya variable de credencial no está definida. Solo comprueba que las
variables están definidas; las claves no válidas aparecen como peticiones fallidas. Consulta
[Validación estricta y permisiva](../configuration/providers.md#strict-and-lenient-validation).

### Registro de costes {#ledger}

`SYNAPSE_LEDGER_BACKENDS` selecciona adónde van las filas de uso. La imagen se compila con
todos los backends, así que cualquier combinación de `sqlite`, `postgres`, `pubsub` y `sns`
funciona sin recompilar:

```bash
docker run -d --name synapse \
  -p 8080:8080 -p 9090:9090 \
  --env-file synapse.env \
  -e SYNAPSE_LEDGER_BACKENDS=postgres \
  -e SYNAPSE_LEDGER_POSTGRES_DSN=postgres://synapse:change-me@db.internal:5432/synapse \
  -v "$(pwd)/config:/app/config:ro" \
  sustentabilitas/synapse-gateway:2.0.0
```

El gateway crea la tabla `usage_events` por sí mismo al arrancar; no hay migraciones que
ejecutar.

El valor por defecto, `sqlite`, escribe `synapse.db` en `/app`, dentro del contenedor, así que
el registro de costes desaparece con el contenedor. Para conservarlo, monta un volumen y apunta
`SYNAPSE_LEDGER_SQLITE_DSN` a él, como hace el inicio rápido con
`sqlite:///app/data/synapse.db?mode=rwc`. El UID 1001 debe poder escribir en el directorio.

Un destino que no puede conectar al arrancar, como un servidor Postgres que aún no está
levantado, se registra en el log y se omite, y el gateway sirve peticiones sin registrarlas en
ese destino. Arranca primero la base de datos y busca `ledger sink connected` en los logs. La
[guía del registro de costes](../guides/cost-ledger.md) explica qué recibe cada destino, y
[Registro de costes](../configuration/environment-variables.md#ledger) enumera todas las
variables de conexión.

### Puertos {#ports}

`SYNAPSE_ADDR` (por defecto `0.0.0.0:8080`) y `SYNAPSE_METRICS_ADDR` (por defecto
`0.0.0.0:9090`) definen las direcciones de escucha dentro del contenedor. El puerto de métricas
solo sirve métricas de Prometheus, en `/metrics` y `/`; todos los demás endpoints, incluido
`/health`, están en el puerto de la API. Publica el puerto de métricas solo en tu red de
monitorización.

`SYNAPSE_ADMIN_ADDR` (por defecto `127.0.0.1:8081`) sirve los endpoints de administración de
A2A. Dentro de un contenedor, `127.0.0.1` es el loopback del propio contenedor, así que nada
fuera de él puede alcanzarlos. Para que los hosts de agentes puedan registrarse, define
`SYNAPSE_ADMIN_ADDR=0.0.0.0:8081` y publica ese puerto solo a ellos, o define
`SYNAPSE_ADMIN_ADDR=shared` para servir los endpoints en el puerto de la API. Consulta
[Seguridad](../operating/security.md#the-a2a-admin-endpoints-are-open).

### Parada {#stopping}

El gateway no instala manejadores de señales y la imagen no tiene proceso init. Linux no
entrega `SIGTERM` al proceso 1 de un contenedor a menos que este gestione la señal, así que
`docker stop` puede agotar su periodo de gracia (10 segundos por defecto) antes de matar el
gateway. Añade `--init` a `docker run`, o `init: true` en Compose, para que el gateway termine
en cuanto reciba la señal.

En cualquier caso, el gateway no hace un cierre ordenado: las peticiones en curso se cortan, y
las filas del registro de costes que aún están en cola en memoria se pierden. Deja de enviar tráfico a una
instancia antes de pararla.

## Construir la imagen {#build-the-image}

Construye cualquiera de las dos imágenes desde la raíz del repositorio:

```bash
docker build -f crates/synapse-gateway/Dockerfile -t synapse-gateway .
docker build -f crates/synapse-proxy/Dockerfile -t synapse-proxy .
```

La compilación del gateway habilita las features `ledger-postgres`, `ledger-pubsub` y
`ledger-sns` además de las predeterminadas, e instala `cmake` y `protobuf-compiler` para ellas.
Para construir una imagen más pequeña con menos backends de registro de costes, cambia la lista
`--features` de
[`crates/synapse-gateway/Dockerfile`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/Dockerfile).

## Ejecutar el proxy {#run-the-proxy}

`synapse-proxy` es un sidecar de proxy inverso independiente, no forma parte del gateway. Su
imagen lee `/app/synapse-proxy.toml`, un ejemplo que sustituyes montando tu propio archivo o
definiendo `SYNAPSE_PROXY_CONFIG_PATH`:

```bash
docker run -d --name synapse-proxy \
  -p 8787:8787 -p 9090:9090 \
  -v "$(pwd)/synapse-proxy.toml:/app/synapse-proxy.toml:ro" \
  sustentabilitas/synapse-proxy:1.0.0
```

El proxy escucha en las direcciones de su archivo de configuración: `addr` (`0.0.0.0:8787` en
el ejemplo) para el tráfico, `admin_addr` (`127.0.0.1:8788`) para su API de administración y
`metrics_addr` (`0.0.0.0:9090`) para las métricas. `SYNAPSE_PROXY_ADDR` sobrescribe `addr`.
Con la API de administración en `127.0.0.1`, solo pueden alcanzarla los procesos dentro del
contenedor, o del mismo pod. A diferencia del gateway, el proxy hace un cierre ordenado al
recibir `SIGTERM`: su sonda de disponibilidad, `GET /healthz/readiness`, empieza a devolver
`503`, y alrededor de un segundo después deja de aceptar conexiones y espera a que terminen las
peticiones en curso.
