# Changelog

## Unreleased

## 1.0.0

## 0.2.3

- Relicensed from AGPL-3.0-only to MPL-2.0 (Mozilla Public License 2.0).

## 0.2.2

- Soft-skip A2A seed agents whose `card_url` is unreachable or invalid after retries (warn + continue). Prevents CrashLoop deadlock when a peer agent is still booting.

## 0.2.1

## 0.2.0

- Static seed from `[[a2a_agents]]` TOML (`seed_from_path`); cards fetched from `card_url` at boot with retry.
- **Breaking:** registration is insert-only (first-writer-wins). Duplicate `POST` / seed id no longer hot-swaps; returns success and keeps the first entry.
- **Breaking:** `try_register` takes `A2aRegistration` instead of many positional arguments.

## 0.1.0

- Initial A2A agent registry: admin register/deregister, public catalog/card/resolve, in-memory TTL registry.
