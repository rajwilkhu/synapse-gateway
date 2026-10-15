---
sidebar_position: 3
title: API de administración
description: Registra y elimina agentes A2A en tiempo de ejecución a través de los endpoints /internal/a2a del gateway.
---

Los hosts de agentes usan la API de administración para añadir sus agentes al registro cuando
arrancan y para eliminarlos cuando se detienen. Los endpoints están en el listener de
administración, `SYNAPSE_ADMIN_ADDR` (por defecto `127.0.0.1:8081`); con `shared` están en el
puerto de la API.

:::warning
Los endpoints de administración no tienen autenticación. Cualquiera que pueda alcanzarlos puede
eliminar un agente y registrar otro endpoint bajo su id. Permite `/internal/` solo desde los
servicios que registran agentes; consulta
[Los endpoints de administración de A2A están abiertos](../../operating/security.md#the-a2a-admin-endpoints-are-open).
:::

## `POST /internal/a2a/agents` {#post-internala2aagents}

Registra un agente si su id todavía no está registrado.

```bash
curl -s -X POST localhost:8081/internal/a2a/agents \
  -H 'Content-Type: application/json' \
  -d @agent.json
```

```json
{
  "id": "invoice-agent",
  "name": "Invoice agent",
  "description": "Extracts line items from invoices",
  "endpoint_url": "https://agents.example.com/invoice",
  "card_url": "https://agents.example.com/invoice/.well-known/agent-card.json",
  "tags": ["finance"],
  "card": {
    "name": "Invoice agent",
    "description": "Extracts line items from invoices",
    "url": "https://agents.example.com/invoice",
    "version": "1.0",
    "skills": []
  },
  "ttl_seconds": 3600
}
```

| Campo | Tipo | Obligatorio | Descripción |
|---|---|---|---|
| `id` | string | Sí | Clave en el registro, usada en los paths `/a2a/agents/{id}/...`. |
| `name` | string | Sí | Nombre visible en el catálogo. |
| `description` | string | Sí | Resumen breve en el catálogo. |
| `endpoint_url` | string | Sí | URL absoluta en la que el agente sirve A2A JSON-RPC. |
| `card_url` | string | Sí | URL absoluta de la agent card del agente, que aparece en el catálogo. |
| `tags` | array of strings | Sí | Etiquetas libres; envía `[]` si no hay ninguna. |
| `card` | any JSON | Sí | La agent card, que se guarda y se devuelve sin cambios. |
| `ttl_seconds` | integer | No | Segundos hasta que el agente caduca. Omítelo para que no caduque. |

El gateway guarda la petición tal cual: no obtiene `card_url` ni comprueba que la agent card
coincida con los demás campos. Mantén `card` como una agent card A2A válida, porque los clientes
la reciben literalmente.

Devuelve `204 No Content`. Un cuerpo que no es JSON, o al que le falta un campo obligatorio, lo
rechaza el parser JSON con un estado `4xx` antes de registrar nada.

### Ids duplicados {#duplicate-ids}

El registro sigue la regla gana el primero que escribe (first-writer-wins). Si el id ya está
registrado, desde el archivo semilla o por un `POST` anterior, la petición se ignora y aun así
devuelve `204`. Se conservan la entrada existente y su caducidad. Para sustituir un agente,
elimínalo primero:

```bash
curl -s -X DELETE localhost:8081/internal/a2a/agents/invoice-agent
curl -s -X POST localhost:8081/internal/a2a/agents -H 'Content-Type: application/json' -d @agent.json
```

Un agente caducado solo se elimina la próxima vez que se lista el catálogo o se consulta el
agente. Hasta entonces, un `POST` con su id se sigue tratando como duplicado. Por eso, un host
de agentes que vuelve a registrarse periódicamente debería hacer siempre `DELETE` antes del
`POST`.

## `DELETE /internal/a2a/agents/{id}` {#delete-internala2aagentsid}

Elimina el agente. Devuelve `204 No Content` tanto si el id estaba registrado como si no, así
que es seguro llamarlo en cada apagado.

## Relacionado {#related}

- [Descubrimiento](./discovery.md): lo que ven los clientes después de un registro.
- [Semilla estática](./static-seed.md): registra agentes al arrancar.
- [API HTTP](../../reference/http-api.md#a2a-agent-registry): los mismos endpoints junto al
  resto de la API del gateway.
