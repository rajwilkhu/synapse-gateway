---
sidebar_position: 1
title: Instalación
description: Instala el gateway Synapse como imagen de Docker, como binario con Cargo o como crate de biblioteca en tu servicio Rust.
---

Puedes ejecutar Synapse desde la imagen de Docker publicada, instalar el binario con Cargo
o embeber el gateway en tu propio servicio Rust como biblioteca. La imagen es la forma más
rápida de probarlo; la biblioteca conviene a los servicios que quieren el enrutamiento y la
contabilidad de costes en el mismo proceso.

## Docker {#docker}

Descarga la imagen de Docker Hub:

```bash
docker pull sustentabilitas/synapse-gateway
```

Se publican etiquetas para cada versión (por ejemplo, `2.0.0`), `latest` para la versión
más reciente y `edge` para la punta de `main`. Las imágenes solo se construyen para
`linux/amd64`. En Apple silicon u otra máquina ARM, añade `--platform linux/amd64` a
`docker pull` y `docker run`, y Docker las ejecutará mediante emulación.

La imagen:

- escucha en el puerto `8080` para la API y en el puerto `9090` para las métricas de
  Prometheus, y sirve los endpoints de administración de A2A en `127.0.0.1:8081`, el loopback
  del propio contenedor (consulta [Docker](../deployment/docker.md#ports));
- se ejecuta como el usuario no root `synapse` (UID 1001), que no tiene directorio home;
- usa `/app` como directorio de trabajo y lee la configuración de `/app/config`;
- incluye todos los backends del registro (SQLite, Postgres, Pub/Sub y SNS).

La imagen incluye la configuración por defecto de
[`crates/synapse-gateway/config/`](https://github.com/sustentabilitas/synapse-gateway/blob/main/crates/synapse-gateway/config).
Monta tu propio directorio sobre `/app/config` para aportar tus rutas, precios y guardrails:

```bash
docker run --rm -p 8080:8080 -p 9090:9090 \
  -v "$(pwd)/config:/app/config" \
  sustentabilitas/synapse-gateway
```

Los tramos de Vertex AI se autentican con las Application Default Credentials de Google.
El usuario del contenedor no tiene directorio home, así que las credenciales de
`gcloud auth application-default login` no están disponibles dentro de él. Monta una clave
de cuenta de servicio y define `GOOGLE_APPLICATION_CREDENTIALS`, o ejecútalo en Google
Cloud, donde el servidor de metadatos proporciona las credenciales. El
[inicio rápido](quickstart.md) muestra el comando completo.

Para construir la imagen tú mismo desde la raíz del repositorio:

```bash
docker build -f crates/synapse-gateway/Dockerfile -t synapse-gateway .
```

## Cargo {#cargo}

Instala el binario `synapse-gateway` desde crates.io:

```bash
cargo install synapse-gateway
```

O constrúyelo desde el código fuente:

```bash
git clone https://github.com/sustentabilitas/synapse-gateway.git
cd synapse-gateway
cargo build --release -p synapse-gateway
```

El binario se genera en `target/release/synapse-gateway`. Lee `config/routes.toml` y
`config/pricing.toml` de forma relativa al directorio de trabajo; define
`SYNAPSE_ROUTES_PATH` y `SYNAPSE_PRICING_PATH` para usar otras rutas de archivo.

Las features de Cargo seleccionan el servidor HTTP y los backends del registro:

| Feature | Por defecto | Activa |
|---|---|---|
| `server` | Sí | El servidor HTTP y el binario `synapse-gateway`. |
| `ledger-sqlite` | Sí | El backend del registro en SQLite. |
| `ledger-postgres` | No | El backend del registro en Postgres. |
| `ledger-pubsub` | No | Publicación de eventos del registro en Google Cloud Pub/Sub. |
| `ledger-sns` | No | Publicación de eventos del registro en AWS SNS. |

Las features se suman a las que vienen por defecto. Por ejemplo:

```bash
# Por defecto: servidor + registro en SQLite
cargo install synapse-gateway

# Añade el registro en Postgres y la publicación en Pub/Sub
cargo install synapse-gateway --features "ledger-postgres ledger-pubsub"

# Solo el registro en Postgres, sin SQLite
cargo install synapse-gateway --no-default-features --features "server ledger-postgres"
```

:::note
Las features de registro en la nube arrastran árboles de dependencias más grandes. La
construcción de Docker instala `cmake` y `protobuf-compiler` para ellas; instala ambos si
falla una compilación con `ledger-pubsub` o `ledger-sns`.
:::

## Como biblioteca {#as-a-library}

Añade el crate sin sus features por defecto, para obtener el gateway sin el servidor HTTP
ni ningún backend del registro:

```toml
[dependencies]
synapse-gateway = { version = "2", default-features = false }
```

El crate de biblioteca se llama `synapse`, así que lo importas como `synapse::…`, por
ejemplo `use synapse::gateway::Gateway;`. Construye un gateway con `Gateway::builder()` y
llama a `Gateway::chat()` en el mismo proceso. Añade `ledger-sqlite`, `ledger-postgres`,
`ledger-pubsub` o `ledger-sns` a `features` si quieres el backend del registro
correspondiente.

[Embeber Synapse como biblioteca](../guides/embedding-as-library.md) recorre un ejemplo
completo, que incluye streaming, embeddings y lo que el builder deja fuera.
[Crates del workspace](../internals/workspace-crates.md#synapse-gateway) describe los
módulos y las features del crate.

La referencia de la API está en [docs.rs](https://docs.rs/synapse-gateway).
