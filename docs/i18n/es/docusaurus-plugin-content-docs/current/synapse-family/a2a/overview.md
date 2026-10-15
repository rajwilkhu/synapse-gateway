---
sidebar_position: 1
title: synapse-a2a
description: synapse-a2a es un registro en memoria de agentes agent-to-agent (A2A) que el gateway Synapse sirve en su puerto de API, con registro por administración en `SYNAPSE_ADMIN_ADDR` y descubrimiento público.
---

`synapse-a2a` es un registro en memoria de agentes agent-to-agent
([A2A](https://a2a-protocol.org/)). Los servicios que alojan agentes los registran, y los
clientes los descubren a través de un catálogo, obtienen sus agent cards y resuelven el id de un
agente a la URL en la que sirve A2A. El registro guarda lo que recibe y lo devuelve tal cual: no
hace de proxy del tráfico A2A ni llama a los agentes después del arranque.

## Cuándo usarlo {#when-to-use-it}

Usa el registro cuando varios hosts de agentes y varios clientes necesiten un lugar estable
donde encontrar agentes por id, y ya ejecutes el gateway Synapse. Los hosts de agentes se
registran por HTTP o figuran en un archivo semilla; los clientes leen el catálogo en la misma
dirección que la API de LLM.

Como el registro vive en la memoria de cada proceso del gateway, encaja con una única instancia
del gateway o con un conjunto de agentes conocido al arrancar. Consulta
[Ejecutar más de una instancia](#running-more-than-one-instance).

## Cómo encaja {#how-it-fits}

El binario del gateway sirve el registro. Al arrancar crea un registro, lo siembra desde
`a2a.toml` si el archivo existe, y sirve las rutas públicas en su listener de API,
`SYNAPSE_ADDR` (por defecto `0.0.0.0:8080`), y las rutas de administración en
`SYNAPSE_ADMIN_ADDR` (por defecto `127.0.0.1:8081`). No hay un proceso aparte:

```text
agent host ──POST   /internal/a2a/agents────────────────▶ synapse-gateway :8081 (admin)
agent host ──DELETE /internal/a2a/agents/{id}───────────▶ same
client     ──GET    /.well-known/a2a-agent-catalog.json─▶ synapse-gateway :8080
client     ──GET    /a2a/agents/{id}/resolve────────────▶ same   ──▶ then calls the agent directly
```

| Método | Path | Propósito |
|---|---|---|
| `POST` | `/internal/a2a/agents` | [Registrar un agente](./admin-api.md). |
| `DELETE` | `/internal/a2a/agents/{id}` | [Eliminar un agente](./admin-api.md). |
| `GET` | `/.well-known/a2a-agent-catalog.json` | [Listar los agentes](./discovery.md). |
| `GET` | `/a2a/agents/{id}/.well-known/agent-card.json` | [La agent card de un agente](./discovery.md). |
| `GET` | `/a2a/agents/{id}/resolve` | [El endpoint y la agent card de un agente](./discovery.md). |

Los mismos endpoints aparecen junto al resto de la API del gateway en
[Registro de agentes A2A](../../reference/http-api.md#a2a-agent-registry). Las rutas A2A no leen
los encabezados de atribución `x-synapse-*` y no quedan anotadas en el registro de costes ni en
las métricas.

:::warning
Los endpoints `/internal/` no tienen autenticación. Mantén `SYNAPSE_ADMIN_ADDR` accesible solo
para los servicios que registran agentes; consulta [Los endpoints de administración de A2A
están abiertos](../../operating/security.md#the-a2a-admin-endpoints-are-open).
:::

## Ejecutar más de una instancia {#running-more-than-one-instance}

Cada proceso del gateway tiene su propio registro, y no se persiste nada. Un agente registrado a
través de la API de administración solo existe en la instancia que recibió la llamada, y todos
los registros se pierden al reiniciar. Detrás de un balanceador de carga, los clientes verían un
catálogo distinto según la instancia que respondiera.

Para mantener las instancias coherentes, enumera los agentes compartidos en
[`a2a.toml`](./static-seed.md), de modo que todas las instancias siembren el mismo catálogo al
arrancar, y bloquea los endpoints de administración. Consulta también
[Limitaciones](../../reference/limitations-roadmap.md#operations).

## Usar el crate {#using-the-crate}

`synapse-a2a` también es una biblioteca. Para servir el registro desde tu propia aplicación
axum, consulta [API del crate](./crate-api.md).
