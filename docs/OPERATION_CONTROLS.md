# Operation Controls

This document covers the shared control primitives for expensive Trellis
operations, failed-operation recovery, semantic validation, and dependency
health.

## Rate Limits and Abuse Controls

Use `shared::quota::check_and_consume` for normal rate limits and
`shared::abuse::record_abuse` for suspicious rejected attempts. Quota state is
keyed by `(actor, resource)`. Abuse state is also keyed by `(actor, resource)`
and decays over ledger windows.

Suggested resource symbols:

| Resource | Use |
| --- | --- |
| `aid_crt` | Aid creation and escrow funding |
| `reb` | Rebalancer execution |
| `claim` | Claim/refund settlement |
| `oracle` | Oracle submissions |

## Recovery Center

When a user-facing operation fails after intent is known, call
`shared::recovery::open_recovery`. Frontends can query records with
`list_recoveries` and show a pending, retried, resolved, or cancelled status.

## Semantic Validation

Use `shared::semantic` before storage writes and cross-contract calls:

- `validate_amount` for min/max amount rules.
- `validate_distinct_parties` for donor/recipient, buyer/seller, and caller/target checks.
- `validate_future_expiry` for bounded future ledger windows.

## Transaction Preflight

Run `shared::preflight` before signing or submitting a high-risk operation
(treasury withdrawal, escrow release/refund, upgrade, migration resume,
rebalance, batch invoke). `preflight` is a pure function of typed facts — it
returns `Ready`, `Warning`, or `Blocked` checks with a user-safe message and a
remediation step, and `require_preflight` guards an entry point with the shared
error codes. See [`docs/PREFLIGHT.md`](./PREFLIGHT.md) for the full check table
and determinism notes.

## Dependency Health

Use `shared::health` to publish contract-visible assumptions about external
dependencies such as RPC, oracle feeds, indexers, and wallets. The health record
tracks latest status, last checked ledger, last successful ledger, and failure
count.
