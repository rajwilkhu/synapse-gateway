---
sidebar_position: 7
title: Atribución por tenant
description: Atribuye cada petición a un tenant, workspace, usuario, hilo y tipo de tarea con encabezados x-synapse-*, y clasifica el trabajo con tipos de tarea de IA.
---

Usa la atribución por tenant cuando más de un equipo, cliente o aplicación comparte un gateway
y necesitas saber quién gastó qué. Los clientes añaden encabezados `x-synapse-*` a sus
peticiones, y Synapse los anota en cada fila del [registro de costes](cost-ledger.md), de modo
que puedes informar del uso y del coste por tenant, por workspace, por usuario final o por
tipo de trabajo sin cambiar el cuerpo de la petición.

## Encabezados de atribución {#attribution-headers}

| Encabezado | Se registra como | Descripción |
|---|---|---|
| `x-synapse-tenant` | `tenant` | El cliente, equipo o aplicación. Sin el encabezado, `SYNAPSE_DEFAULT_TENANT` (por defecto `unattributed`). |
| `x-synapse-workspace` | `workspace` | Una agrupación dentro del tenant, como un proyecto o un equipo. |
| `x-synapse-user` | `user_id` | El usuario final dentro del tenant. |
| `x-synapse-thread` | `thread_id` | Una conversación o un hilo de agente. |
| `x-synapse-message` | `message_id` | Un mensaje o unidad de trabajo dentro del hilo. También se convierte en el id de la petición; consulta [Correlacionar peticiones](#correlate-requests). |
| `x-synapse-user-task-type` | `user_task_type` | Tu propia etiqueta para el trabajo al que sirve la petición, como `summarisation`. Se registra tal cual se envía y nunca se interpreta. |
| `x-synapse-ai-task-type` | `ai_task_type` | Sustituye el [tipo de tarea de IA](#ai-task-types) para esta petición. |

Todos los encabezados son opcionales. Funcionan igual en `POST /v1/chat/completions`,
`POST /v1/embeddings` y los endpoints passthrough. Por ejemplo:

```bash
curl -s http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "x-synapse-tenant: my-team" \
  -H "x-synapse-workspace: onboarding" \
  -H "x-synapse-user: user-42" \
  -H "x-synapse-user-task-type: summarisation" \
  -d '{
    "model": "chat",
    "messages": [{"role": "user", "content": "Summarise our onboarding guide."}]
  }'
```

Synapse registra los valores tal cual se envían. Un encabezado presente pero vacío registra
una cadena vacía en lugar de recurrir al valor por defecto.

:::warning
Synapse no autentica estos encabezados: cualquier cliente puede declarar cualquier tenant. Si
la atribución determina la facturación o las cuotas, ejecuta Synapse detrás de un gateway o
una service mesh que autentique a quienes llaman y establezca `x-synapse-tenant` por sí mismo,
sobrescribiendo cualquier valor que haya enviado el cliente.
:::

## Adónde va la atribución {#where-attribution-goes}

Cada fila del registro tiene una columna para cada uno de los siete campos. `tenant` y
`ai_task_type` siempre tienen valor; los demás son nulos cuando no se envió el encabezado. Los
eventos publicados en Pub/Sub o SNS llevan los mismos campos en camelCase, omitiendo los
ausentes, con el tenant como `namespace`. Consulta [Registro de costes](cost-ledger.md) para el
formato completo de filas y eventos.

Ninguno de ellos es una etiqueta de métrica. Sus valores vienen de los clientes y no están
acotados, y ponerlos en las métricas crearía una serie nueva por cada tenant, usuario o hilo.
Consulta el registro para obtener cifras por tenant, y usa las métricas para vistas por ruta,
por modelo y por carril.

Da a `SYNAPSE_DEFAULT_TENANT` un valor reconocible, como el nombre del despliegue, para que el
uso sin atribuir destaque. Consulta
[Variables de entorno](../configuration/environment-variables.md#tenancy).

## Correlacionar peticiones {#correlate-requests}

Cada petición recibe un id de petición, que se registra como `request_id` en el registro. En
los chat completions es también el `id` de la respuesta, como `chatcmpl-<request id>`. Cuando
la petición tiene un encabezado `x-synapse-message`, ese valor es el id de la petición; si no,
Synapse genera un UUID.

Estampar `x-synapse-message` te permite unir una fila del registro con el mensaje en tu propio
sistema. Todas las filas que escribe una misma petición comparten su id de petición: en una
ruta `strategy = "jev"`, la decisión de enrutamiento y el chat completion; con extracción
híbrida, la respuesta de Jev y cada extracción. Reutilizar un mismo id de mensaje en varias
peticiones da a sus filas el mismo `request_id`.

## Tipos de tarea de IA {#ai-task-types}

El tipo de tarea de IA clasifica el trabajo que realizó el gateway, como `conversation`,
`extraction` o `vision`, para que puedas informar del coste por tipo de trabajo entre rutas y
tenants. Cada fila del registro tiene uno. Synapse lo resuelve por petición:

1. el encabezado `x-synapse-ai-task-type`, si está presente y no está vacío;
2. si no, el tipo de tarea asignado al alias de ruta de la petición en `ai_task_types.toml`;
3. si no, `simple`.

`ai_task_types.toml` se indexa por tipo de tarea, y cada uno enumera los alias de ruta que
realizan ese tipo de trabajo:

```toml
conversation = ["chat", "support-bot"]
extraction = ["invoice-extract"]
vision = ["receipt-ocr"]
```

- El gateway lo lee de `SYNAPSE_AI_TASK_TYPES_PATH` (por defecto
  `config/ai_task_types.toml`) al arrancar. El archivo es opcional: sin él, todas las
  peticiones se resuelven a `simple` salvo que el encabezado diga otra cosa.
- Los alias no listados se resuelven a `simple`, así que enumera solo las rutas cuyo trabajo
  no es simple.
- Un alias listado bajo dos tipos de tarea detiene el gateway al arrancar; listarlo dos veces
  bajo el mismo tipo está permitido.
- En los embeddings, el alias es el alias de embeddings.

Los dos campos de tipo de tarea responden a preguntas distintas. `user_task_type` es la
etiqueta propia de tu aplicación, y solo se registra cuando un cliente la envía.
`ai_task_type` siempre tiene valor y se controla de forma centralizada en
`ai_task_types.toml`, así que los informes se mantienen coherentes aunque los clientes no
envíen nada.

## Multitenencia con organizaciones y workspaces {#multi-tenancy-with-organisations-and-workspaces}

En despliegues multitenant donde varias organizaciones de clientes (`tenant` / `org`), equipos o
proyectos (`workspace`) y usuarios finales comparten Synapse, los crates del workspace trabajan
conjuntamente para **vincular**, **aplicar**, **proteger** y **atribuir** cada llamada a lo largo
de la jerarquía de dos niveles `(organización, workspace)`.

### Jerarquía de tenencia y correspondencia entre crates {#tenancy-hierarchy-and-cross-crate-mapping}

```text
Organización (`tenant` / `org` / `namespace`)          p. ej. "acme"
  └── Workspace (`workspace`)                          p. ej. "onboarding", "prod-support"
        └── Usuario (`user_id` / `user`)               p. ej. "user-42", "agent-triage"
              └── Hilo (`thread_id` / `threadId`)      p. ej. "thread-9001"
                    └── Mensaje (`message_id` / `request_id`) p. ej. "msg-001"
```

| Nivel | Encabezado de `synapse-gateway` | Campo en `RequestCtx` | Columna en `usage_events` (SQLite / Postgres) | Campo JSON en Pub/Sub y SNS | Clave en `synapse-context` / `synapse-proxy` / `synapse-mcp` |
|---|---|---|---|---|---|
| **Organización** | `x-synapse-tenant` | `tenant` | `tenant` (por defecto `SYNAPSE_DEFAULT_TENANT`) | `namespace` (también atributo de filtro del mensaje) | `org` |
| **Workspace** | `x-synapse-workspace` | `workspace` | `workspace` | `workspace` | `workspace` |
| **Usuario** | `x-synapse-user` | `user` | `user_id` | `user` | `user` |
| **Hilo** | `x-synapse-thread` | `thread` | `thread_id` | `threadId` | — |
| **Mensaje** | `x-synapse-message` | `message` | `message_id` y `request_id` | `messageId` y `requestId` | — |

### Aplicación de la identidad de organización y workspace {#enforcing-org-and-workspace-identity}

Como `synapse-gateway` confía en los encabezados `x-synapse-*` que recibe, los clientes no
confiables (clientes externos, sandboxes de código o contenedores de agentes) nunca deben llegar
directamente al gateway. Utiliza uno de estos dos patrones de aplicación:

1. **API gateway o ingress perimetral (API HTTP multitenant)**: Tu ingress o API gateway
   autentica el token del cliente, elimina cualquier encabezado `x-synapse-*` enviado por el
   cliente y estampa `x-synapse-tenant: <org>` y `x-synapse-workspace: <workspace>` antes de
   reenviar la petición a `synapse-gateway`.
2. **Sandbox no confiable o sidecar de agente (`synapse-context` + `synapse-proxy` + `synapse-mcp`)**:
   Tu plano de control vincula el contexto activo `(org, workspace, user)` en un `ContextStore`
   compartido mediante `POST /internal/bind`. Tanto `synapse-proxy` (para llamadas a LLM y HTTP)
   como `synapse-mcp` (para llamadas a herramientas MCP) leen ese almacén, sobrescriben cualquier
   encabezado falsificado por el sandbox y **fallan en cerrado (fail closed)** cuando el contexto
   no está vinculado.

```text
Plano de control
  │  POST 127.0.0.1:8788/internal/bind
  │  {"values": {"org": "acme", "workspace": "onboarding", "user": "user-42"}, "ttl_seconds": 3600}
  ▼
┌────────────────────────────────────────────────────────────────────────────┐
│ `ContextStore` compartido (`synapse-context`)                              │
│   base (static ⊕ env) + overlay con TTL activo → ResolvedContext           │
└───────────────────┬────────────────────────────────────┬───────────────────┘
                    │                                    │
                    ▼                                    ▼
┌──────────────────────────────────────┐  ┌──────────────────────────────────┐
│ `synapse-proxy` (:8787)              │  │ `synapse-mcp` (/mcp/{server})    │
│ • `require_context = ["org",         │  │ • `required = true` en `org` y   │
│    "workspace"]` (503 si no hay)     │  │   `workspace` (falla en cerrado) │
│ • Sobrescribe encabezados con:       │  │ • Abre conexión dedicada por     │
│   `x-synapse-tenant: acme`           │  │   `(server, identity)` con:      │
│   `x-synapse-workspace: onboarding`  │  │   `x-org-id: acme`               │
│   `x-synapse-user: user-42`          │  │   `x-workspace-id: onboarding`   │
└───────────────────┬──────────────────┘  └─────────────────┬────────────────┘
                    │                                       │
                    ▼                                       ▼
┌──────────────────────────────────────┐          Servidores MCP upstream
│ `synapse-gateway` (:8080)            │
│ • Guardrails (`isolation_bypass` y   │
│   `data_exfil`) bloquean fugas entre │
│   organizaciones o workspaces        │
│ • Registro de costes anota           │
│   `tenant="acme"` y                  │
│   `workspace="onboarding"`           │
└──────────────────────────────────────┘
```

Ejemplo de `synapse-proxy.toml` que estampa `org` y `workspace` en las peticiones reenviadas a
`synapse-gateway`:

```toml title="synapse-proxy.toml"
addr = "127.0.0.1:8787"
admin_addr = "127.0.0.1:8788"
metrics_addr = "127.0.0.1:9091"

[context]
env = { org = "TENANT_ORG_ID", workspace = "TENANT_WORKSPACE_ID" }

[[routes]]
name = "llm-gateway"
path_prefix = "/v1"
upstream = "http://127.0.0.1:8080"
strip_prefix = false
require_context = ["org", "workspace"]
request_steps = [
  { inject = { header = "x-synapse-tenant",    from_context = "org" } },
  { inject = { header = "x-synapse-workspace", from_context = "workspace" } },
  { inject = { header = "x-synapse-user",      from_context = "user" } },
]
```

Consulta [Contexto y transformaciones](../synapse-family/proxy/context-and-transforms.md) e
[Inyección de identidad](../synapse-family/mcp/identity-injection.md).

### Protección frente a fugas entre organizaciones y workspaces {#guarding-against-cross-tenant-and-cross-workspace-escapes}

Combina la inyección de encabezados con los escáneres multitenant en `config/guardrails.toml`
para que un modelo o la salida de una herramienta manipulada por inyección de prompt no pueda
emitir instrucciones o comandos que crucen los límites de organización o workspace:

- **[`isolation_bypass`](../configuration/guardrails-policy.md#isolation_bypass)**: Detecta
  intentos en prompts o respuestas del modelo de fijar encabezados de identidad (`x-tenant-id`,
  `x-workspace-id`, `x-user-id`, `x-agent-id`) mediante `curl -H`, literales de diccionario o
  llamadas a métodos setter, o de alcanzar servicios internos de Kubernetes
  (`*.svc.cluster.local`).
- **[`data_exfil`](../configuration/guardrails-policy.md#data_exfil)**: Detecta recorridos no
  acotados de workspaces, como URIs raíz `cortex://<tenant>/<workspace>/` o `recursive=true` en
  `/fs/ls`.

```toml title="config/guardrails.toml"
[guardrails.multi-tenant]
mode = "block"
scanners = [
  "prompt_injection",
  { type = "secrets", action = "mask" },
  { type = "pii", action = "mask" },
  "isolation_bypass",
  "data_exfil",
]
output_scanners = [
  { type = "secrets", action = "mask" },
  "isolation_bypass",
  "data_exfil",
]
```

### Consultar el gasto por organización y workspace {#querying-spend-by-organisation-and-workspace}

Desglose del gasto diario y del uso de tokens por organización (`tenant`) y `workspace`:

```sql
SELECT tenant AS org,
       COALESCE(workspace, '(none)') AS workspace,
       substr(ts, 1, 10) AS day,
       COUNT(*) AS requests,
       SUM(input_tokens) AS input_tokens,
       SUM(output_tokens) AS output_tokens,
       SUM(cost_usd) AS usd
FROM usage_events
WHERE status = 'ok'
GROUP BY tenant, workspace, day
ORDER BY day DESC, usd DESC;
```

Recuento de peticiones bloqueadas por guardrails por organización y workspace:

```sql
SELECT tenant AS org,
       COALESCE(workspace, '(none)') AS workspace,
       model AS policy_or_classifier,
       COUNT(*) AS blocked_requests
FROM usage_events
WHERE lane = 'guard' AND status = 'blocked'
GROUP BY tenant, workspace, model
ORDER BY blocked_requests DESC;
```

## En un gateway embebido {#in-an-embedded-gateway}

Las aplicaciones que embeben el gateway pasan los mismos campos en un `RequestCtx`, cuyos
campos `tenant`, `workspace`, `user`, `thread`, `message`, `user_task_type` y `ai_task_type`
corresponden a los encabezados, más un `request_id` opcional. Consulta
[Embeber Synapse como biblioteca](embedding-as-library.md).
