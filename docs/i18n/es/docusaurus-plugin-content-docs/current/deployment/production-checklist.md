---
sidebar_position: 3
title: Lista de comprobación para producción
description: Qué decidir y configurar antes de que el gateway Synapse sirva tráfico real, desde las etiquetas de imagen y las credenciales hasta el registro de costes, las métricas y la autenticación de entrada.
---

Recorre esta lista antes de que el gateway sirva tráfico real. Cada punto enlaza con la página
que explica el ajuste en detalle.

## Imagen y configuración {#image-and-configuration}

- [ ] **Fija la etiqueta de la imagen.** Ejecuta una etiqueta de versión como
  `sustentabilitas/synapse-gateway:2.0.0`, no `latest` ni `edge`, para que un reinicio nunca
  cambie la versión que ejecutas. Lee el registro de cambios antes de cambiar de etiqueta.
  Consulta [Docker](docker.md#images).
- [ ] **Monta tu propio `/app/config`.** La configuración integrada en la imagen es un
  ejemplo. Guarda tus `routes.toml`, `pricing.toml` y archivos opcionales en control de
  versiones y móntalos en solo lectura. El gateway los lee una sola vez al arrancar y no tiene
  endpoint de recarga, así que un cambio requiere reiniciar. Consulta
  [Docker](docker.md#configuration).
- [ ] **Mantén la validación estricta de proveedores.** Deja `SYNAPSE_PROVIDER_VALIDATION` sin
  definir o en `strict`, para que una credencial ausente detenga el gateway al arrancar en
  lugar de eliminar tramos de tus rutas sin avisar. Consulta
  [Validación estricta y permisiva](../configuration/providers.md#strict-and-lenient-validation).
- [ ] **Prueba cada tramo.** Un tramo mal configurado en el carril estándar hace fallback sin
  avisar en cada petición, lo que añade latencia. Llama a cada proveedor y modelo mediante una
  ruta de un solo tramo antes de depender de una cadena. Consulta
  [Cadenas de fallback](../guides/fallback-chains.md#design-a-chain).
- [ ] **Mantén los precios al día.** Un modelo de chat que falta en `pricing.toml` se registra
  con coste 0. Consulta [Precios](../configuration/pricing.md).

## Credenciales {#credentials}

- [ ] **Guarda las claves de los proveedores como secretos.** Pásalas desde el almacén de
  secretos de tu orquestador o con un `--env-file`, nunca integradas en una imagen ni escritas
  en una línea de comandos.
- [ ] **Dale a Vertex AI su propia identidad.** Usa Workload Identity o el servidor de
  metadatos en Google Cloud, o, en otros entornos, una clave de cuenta de servicio limitada a
  llamar a Vertex AI. Consulta [Credenciales](docker.md#credentials).

## Tiempos de espera {#timeouts}

- [ ] **Dimensiona `SYNAPSE_REQUEST_TIMEOUT_SECS` para tu respuesta más larga.** Limita el
  primer fragmento en el carril estándar y, como tiempo de espera HTTP del proveedor, cada
  respuesta completa, en todos los carriles. Las salidas estructuradas largas o el razonamiento
  intensivo necesitan más que los 120 segundos por defecto. Consulta
  [Tiempos de espera](../guides/streaming-and-tools.md#timeouts).
- [ ] **Ten en cuenta la cadena.** Cada tramo puede consumir el tiempo de espera completo antes
  de que empiece el siguiente, así que una cadena de tres tramos puede tardar tres veces más en
  fallar. Configura los tiempos de espera de tu balanceador de carga y de tus clientes por
  encima de eso.

## Registro de costes {#ledger}

- [ ] **Elige un backend de registro de costes.** El registro SQLite por defecto es un archivo
  en una sola máquina, dentro del contenedor salvo que montes un volumen. Sirve para una sola
  instancia. Con más de una réplica, usa `postgres`, y añade `pubsub` o `sns` para el uso que
  no puedas perder. Consulta [Registro de costes](../guides/cost-ledger.md#sinks).
- [ ] **Comprueba que todos los destinos han conectado.** Un destino que falla al arrancar se
  omite y el gateway sigue sirviendo sin él. Busca `ledger sink connected` en los logs de
  arranque para cada backend.
- [ ] **Configura alertas sobre filas perdidas.** `synapse_ledger_dropped_total` cuenta las
  filas descartadas porque la cola estaba llena; `synapse_ledger_errors_total` cuenta las
  escrituras fallidas. Ambas deberían mantenerse en 0. Consulta
  [Entrega](../guides/cost-ledger.md#delivery).

## Observabilidad {#observability}

- [ ] **Recoge las métricas.** Haz scraping de `:9090` con Prometheus, o define
  `OTEL_EXPORTER_OTLP_ENDPOINT` para que el gateway envíe las métricas a tu colector. Mantén
  el puerto de métricas fuera de la red pública. Consulta
  [Variables de entorno](../configuration/environment-variables.md#telemetry).
- [ ] **Mide los errores fuera del gateway.** Las métricas de peticiones de Synapse cuentan las
  peticiones que produjeron una respuesta; las peticiones que fallan con un `4xx` o un `5xx` no
  se cuentan. Obtén las tasas de error de tu balanceador de carga, ingress o service mesh.
- [ ] **Envía los logs.** El gateway escribe los logs en la salida estándar en texto plano.
  Define `RUST_LOG=info` (el valor por defecto) y `NO_COLOR=1` para que tu recolector de logs
  reciba texto sin códigos de color.

## Seguridad {#security}

- [ ] **Pon delante un proxy con autenticación.** Synapse no tiene autenticación de entrada ni
  limitación de tasa: cualquiera que pueda alcanzar el puerto `8080` puede gastar tu
  presupuesto de proveedores. Ejecútalo detrás de un API gateway, ingress o service mesh que
  autentique a quienes llaman y limite su tasa de peticiones.
- [ ] **Define el tenant en el proxy.** Los clientes pueden enviar cualquier
  `x-synapse-tenant`. Si la atribución importa, haz que tu proxy defina el encabezado a partir
  de la identidad autenticada, sobrescribiendo el valor del cliente. Consulta
  [Atribución por tenant](../guides/tenant-attribution.md).
- [ ] **Bloquea los endpoints internos.** Los endpoints de administración del registro A2A,
  `POST /internal/a2a/agents` y `DELETE /internal/a2a/agents/{id}`, se sirven en
  `SYNAPSE_ADMIN_ADDR`. Déjalo sin definir (loopback) o accesible solo desde los hosts de
  agentes; evita `shared`.
- [ ] **Protege todos los carriles.** Define una política `default` o una política por ruta y
  por alias de embeddings. Mantén `SYNAPSE_GUARDRAILS=required` y
  `SYNAPSE_PASSTHROUGH_MODELS=routed`.
- [ ] **Configura guardrails.** Añade un `guardrails.toml` con una política `default`,
  desplegada primero en modo `observe`, para que las inyecciones de prompt, los secretos y los
  datos personales se detecten antes de llegar a un proveedor. Consulta
  [Política de guardrails](../configuration/guardrails-policy.md).
- [ ] **Define `SYNAPSE_DEFAULT_TENANT`.** Asigna a las peticiones sin encabezado de tenant un
  tenant reconocible, como el nombre del despliegue, en lugar de `unattributed`, para que el
  gasto sin atribuir destaque en el registro de costes.

## Ejecutar más de una instancia {#running-more-than-one-instance}

- [ ] **Comparte el registro de costes.** Cada réplica escribe sus propias filas; apúntalas
  todas a la misma base de datos Postgres o al mismo topic de mensajería.
- [ ] **Siembra el registro A2A desde un archivo.** El registro está en memoria, por
  instancia: un agente registrado mediante `POST /internal/a2a/agents` solo existe en la
  réplica que recibió la llamada, y todos los registros se pierden al reiniciar. En su lugar,
  siembra los agentes desde `a2a.toml` en todas las réplicas.
- [ ] **Drena antes de parar.** El gateway no termina las peticiones en curso al apagarse.
  Retira una instancia de tu balanceador de carga antes de pararla, y usa un proceso init para
  que termine enseguida. Consulta [Parada](docker.md#stopping).
- [ ] **Sondea `/health`.** Devuelve `200` con el cuerpo `ok` en cuanto el listener de la API
  está activo.
