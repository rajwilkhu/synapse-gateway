---
sidebar_position: 3
title: Admin API
description: Register and remove A2A agents at runtime through the gateway's /internal/a2a endpoints.
---

Agent hosts use the admin API to add their agents to the registry when they start, and to
remove them when they stop. The endpoints are on the admin listener, `SYNAPSE_ADMIN_ADDR`
(default `127.0.0.1:8081`); with `shared` they are on the API port.

:::warning
The admin endpoints have no authentication. Anyone who can reach them can remove an agent
and register another endpoint under its id. Allow `/internal/` only from the services that
register agents; see
[The A2A admin endpoints are open](../../operating/security.md#the-a2a-admin-endpoints-are-open).
:::

## `POST /internal/a2a/agents`

Registers an agent if its id isn't registered yet.

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

| Field | Type | Required | Description |
|---|---|---|---|
| `id` | string | Yes | Registry key, used in the `/a2a/agents/{id}/...` paths. |
| `name` | string | Yes | Display name in the catalogue. |
| `description` | string | Yes | Short summary in the catalogue. |
| `endpoint_url` | string | Yes | Absolute URL where the agent serves A2A JSON-RPC. |
| `card_url` | string | Yes | Absolute URL of the agent's card, listed in the catalogue. |
| `tags` | array of strings | Yes | Free-form tags; send `[]` for none. |
| `card` | any JSON | Yes | The agent card, stored and served back unchanged. |
| `ttl_seconds` | integer | No | Seconds until the agent expires. Omit it for no expiry. |

The gateway stores the request as it is: it doesn't fetch `card_url` or check that the card
matches the other fields. Keep `card` a valid A2A agent card, because clients receive it
verbatim.

Returns `204 No Content`. A body that isn't JSON, or is missing a required field, is rejected
by the JSON parser with a `4xx` status before anything is registered.

### Duplicate ids

Registration is first-writer-wins. If the id is already registered, from the seed file or an
earlier `POST`, the request is ignored and still returns `204`. The existing entry, and its
expiry, are kept. To replace an agent, delete it first:

```bash
curl -s -X DELETE localhost:8081/internal/a2a/agents/invoice-agent
curl -s -X POST localhost:8081/internal/a2a/agents -H 'Content-Type: application/json' -d @agent.json
```

An expired agent is only removed the next time the catalogue is listed or the agent is looked
up. Until then, a `POST` with its id is still treated as a duplicate. An agent host that
re-registers on a timer should therefore always `DELETE` before it `POST`s.

## `DELETE /internal/a2a/agents/{id}`

Removes the agent. Returns `204 No Content` whether or not the id was registered, so it is
safe to call on every shutdown.

## Related

- [Discovery](./discovery.md): what clients see after a registration.
- [Static seed](./static-seed.md): register agents at startup instead.
- [HTTP API](../../reference/http-api.md#a2a-agent-registry): the same endpoints alongside the
  rest of the gateway's API.
