---
sidebar_position: 4
title: Seguridad
description: Qué protege y qué no protege el gateway, cómo desplegarlo de forma segura y cómo informar de una vulnerabilidad.
---

El gateway guarda las credenciales de tus proveedores y gasta dinero en cada llamada, pero no
autentica a quienes lo llaman. Trátalo como un servicio interno: colócalo en una red privada,
detrás de algo que decida quién puede llamarlo. Esta página lista lo que el gateway deja en tus
manos. La [lista de comprobación para producción](../deployment/production-checklist.md#security)
lo convierte en pasos.

## Informar de una vulnerabilidad {#report-a-vulnerability}

No informes de vulnerabilidades de seguridad en issues, discusiones ni pull requests públicos de
GitHub. Infórmalas de forma privada, de una de estas dos maneras:

- abre un borrador de aviso en la pestaña **Security → Advisories** del repositorio
  (preferible), o
- escribe a [raj@sustentabilitas.com](mailto:raj@sustentabilitas.com).

Incluye la versión o el commit afectados, una descripción del problema y su impacto, los pasos
para reproducirlo o una prueba de concepto, y la configuración y los logs relevantes, sin
secretos. Recibirás un acuse de recibo en un plazo de tres días hábiles. El proyecto sigue la
divulgación coordinada: da a los mantenedores un plazo razonable para publicar una corrección
antes de hacerlo público, e indica si quieres que se te reconozca el hallazgo. Las
vulnerabilidades de las dependencias corresponden a esos proyectos, pero los mantenedores
ayudarán a coordinar si una afecta al gateway.

La política completa está en
[SECURITY.md](https://github.com/sustentabilitas/synapse-gateway/blob/main/SECURITY.md).

## Los clientes no se autentican {#callers-arent-authenticated}

El gateway no tiene claves de API, ni autenticación de entrada, ni limitación de tasa. Cualquiera
que pueda alcanzar el listener puede:

- llamar a cualquier ruta, y gastar tu cuota y tu dinero con los proveedores;
- llamar al [passthrough de Gemini](../guides/native-vertex.md#gemini-sdk-clients) con cualquier
  nombre de modelo de Vertex AI, incluidos modelos que ninguna ruta lista. Un modelo que la tabla
  de rutas no conoce se reenvía tal cual, con las credenciales de Google del gateway;
- llamar al passthrough de Jev, `POST /typesafe/v1/systemone`, cuando `TYPESAFE_API_KEY` está
  definida;
- registrar y eliminar agentes A2A (consulta
  [los endpoints de administración de A2A](#the-a2a-admin-endpoints-are-open)).

Coloca delante un proxy inverso o un API gateway que autentique, y aplica ahí los límites de tasa
y los presupuestos. El gateway sirve HTTP sin cifrar; termina TLS en ese mismo proxy o en tu
service mesh.

## Los encabezados de tenant se aceptan sin comprobar {#tenant-headers-are-trusted}

Los [encabezados de atribución](../guides/tenant-attribution.md#attribution-headers), como
`x-synapse-tenant` y `x-synapse-user`, van directamente al registro de costes. El gateway no los
comprueba, así que un cliente puede cargar su uso a otro tenant enviando el nombre de ese tenant.
Si facturas o fijas presupuestos a partir del registro, haz que tu proxy elimine los encabezados
de atribución que envíe el cliente y los defina a partir de su identidad autenticada.

## Los endpoints de administración de A2A están abiertos {#the-a2a-admin-endpoints-are-open}

El gateway sirve un catálogo de agentes A2A. Dos de sus endpoints modifican el catálogo y no
tienen autenticación:

- `POST /internal/a2a/agents` registra un agente;
- `DELETE /internal/a2a/agents/{id}` elimina uno.

Por defecto escuchan en `127.0.0.1:8081`, aparte del puerto de la API. En un host, solo los
procesos locales pueden alcanzarlos. En Docker o Kubernetes, `127.0.0.1` es el loopback del
propio contenedor, así que nada fuera del contenedor puede alcanzarlos. Para que los hosts de
agentes puedan registrarse, define `SYNAPSE_ADMIN_ADDR=0.0.0.0:8081` y publica o expón ese
puerto solo a ellos, o define `SYNAPSE_ADMIN_ADDR=shared` para servir los endpoints en el
puerto de la API.

Registrar un agente nunca sobrescribe un id existente, pero un cliente puede eliminar un agente
y registrar otro con el mismo id y su propio endpoint. Las peticiones de los clientes que
resuelven agentes a través del catálogo acabarían entonces en ese endpoint. Bloquea `/internal/` en tu proxy para todo
salvo los servicios que registran agentes, o carga el catálogo desde `a2a.toml` al arrancar y
bloquea por completo los endpoints de administración.

## El puerto de métricas no tiene autenticación {#the-metrics-port-is-unauthenticated}

El listener de métricas, `SYNAPSE_METRICS_ADDR` (por defecto `0.0.0.0:9090`), sirve `/metrics` a
cualquiera que pueda alcanzarlo. Las series nombran tus rutas, los modelos upstream, las
políticas de guardrails y los niveles de Jev. Expón el puerto solo a tu Prometheus.

## Credenciales {#credentials}

El gateway lee las credenciales de los proveedores de variables de entorno y del archivo de la
cuenta de servicio al que apunta `GOOGLE_APPLICATION_CREDENTIALS`.
[Variables de entorno](../configuration/environment-variables.md#providers) las lista todas.

- Inyéctalas desde un almacén de secretos, no desde archivos incluidos en una imagen o
  subidos a un repositorio.
- Para Vertex AI, prefiere una workload identity a un archivo de clave de cuenta de servicio:
  Application Default Credentials la detecta sin ninguna clave que se pueda filtrar.
- Da a cada credencial solo el acceso que necesita el gateway. El passthrough de Gemini puede
  llegar a cualquier modelo de Vertex AI que la identidad de Google pueda llamar, así que los
  permisos de esa identidad son el límite real de lo que pueden usar los clientes.
- Los DSN del registro de costes contienen contraseñas de bases de datos. Guárdalos también en
  el almacén de secretos.

El gateway nunca registra credenciales en los logs y no devuelve las credenciales de los
proveedores a los clientes.

## Contenido {#content}

El gateway no registra prompts ni respuestas generadas, y el registro de costes no almacena contenido:
cada fila contiene identificadores, la ruta, el proveedor y el modelo, los recuentos de tokens,
el coste y el estado. Consulta [Qué se registra](./logging.md#what-is-logged) y el
[formato de fila](../guides/cost-ledger.md#row-format).

Los cuerpos de error de los proveedores son distintos: cuando fallan todos los tramos, el gateway
devuelve al cliente el error de cada tramo, y el mensaje de error de un proveedor puede citar
parte de la petición.

## Guardrails {#guardrails}

Las [políticas de guardrails](../configuration/guardrails-policy.md) escanean el texto de todos
los carriles (chat, passthrough de Gemini, embeddings y passthrough de Jev) antes de que ningún
proveedor lo vea, con el `policy` de la ruta o, si no, `default`; consulta [Qué se
escanea](../configuration/guardrails-policy.md#what-is-scanned). Una política en modo `block`
rechaza con un `400` la petición que coincide, indicando la política y los escáneres que
coincidieron; consulta [Respuesta de
bloqueo](../configuration/guardrails-policy.md#block-response). Las políticas son una primera
línea de defensa basada en patrones, no un filtro completo:

- solo escanean respuestas en las chat completions bajo una política con
  [`output_scanners`](../configuration/guardrails-policy.md#output-scanning); las respuestas
  del passthrough, de embeddings y de Jev nunca se escanean;
- una ruta sin `policy` usa la política llamada `default`, y no tiene protección si no defines
  una;
- la respuesta de bloqueo indica al cliente qué escáneres coincidieron, lo que ayuda a un
  atacante a ajustar un prompt hasta que pase.

## Tamaño de la petición {#request-size}

Los cuerpos de las peticiones se analizan como JSON, con el límite por defecto de axum de 2 MB.
Los cuerpos más grandes se rechazan antes de llegar a un proveedor. El límite no es
configurable, así que envía los medios grandes a Vertex AI por referencia (por ejemplo, con
`vertex.media_uris` en el carril nativo) en lugar de incrustarlos.
