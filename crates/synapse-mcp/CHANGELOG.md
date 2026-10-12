# Changelog

## Unreleased

## 1.0.0

- Relicensed from AGPL-3.0-only to MPL-2.0 (Mozilla Public License 2.0).

## 0.1.4


## 0.1.3

- Bound upstream MCP connect, `list_tools`, and `call_tool` with a 30s timeout; return a stable `upstream MCP call timed out` error instead of hanging the sandbox-facing SSE stream.

## 0.1.2


## 0.1.1


## 0.1.0

- Initial release. On-demand MCP gateway for the `synapse-proxy` sandbox broker: transparent per-server routing over Streamable HTTP, a TTL-scoped upstream registry with admin routes, and per-session tenant-identity injection driven by a generic `McpGatewayConfig` (context-key → header mapping) resolved from the shared `ContextStore`. Loopback-only with DNS-rebinding protection.
