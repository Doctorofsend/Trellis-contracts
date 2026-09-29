# Maintainer audit trail

Sensitive maintainer and service mutations are recorded in the contract's
append-only action audit store in the same Soroban invocation as the state
change. If the audit write fails, the invocation fails rather than committing
an unaudited mutation.

## Covered actions

| Action | Actor | Recorded context |
|---|---|---|
| Treasury initialization, manager grants/revocations, limits, deposits, withdrawals, emergency withdrawals, referral routing, rewards, and quota overrides | Admin, treasury manager, or registered referral service | Resource/attribute and relevant state before/after |
| Access-control initialization, admin/role changes, role hierarchy, and invitation lifecycle | Authenticated admin, inviter, or invitee | Actor, target, role, action, and membership state |
| Aid pauser grants/revocations | Admin | Target, role, and membership state |
| Referral manager grants/revocations | Admin | Target, role, and membership state |
| Oracle submitter registration/deactivation | Admin | Submitter and active state before/after |

The audit entry also includes the treasury scope, a stable action label, a
stable reason code, the ledger sequence, and ledger timestamp. Audit context
does not include free-form input, secrets, token destination addresses, or
recipient details. The affected token or administrative subject is retained
only where needed to identify the resource being changed. Existing ledger
events remain available for transaction details and indexing.

Every new entry is also emitted as a structured `("timeline", "audit_v2")`
ledger event, preserving the full record for off-chain export and long-term
review. The on-contract query reads the newest active persistent entries;
Soroban storage TTLs still apply to those queryable copies.

## Reading entries

The treasury and access-control contracts expose `audit_trail(maintainer,
limit)` for structured action records. The caller must authenticate and hold
the shared `Admin` role; a legacy stored admin address is also accepted during
upgrade compatibility. Results are returned newest first; a limit of zero
selects the default page size and requests above the maximum are rejected. The
legacy `shared::timeline::audit_trail` accessor remains available for the
original audit-record schema. Both stores use separate keys and neither can be
returned from the participant-facing timeline.

The shared timeline API can also be used by other contract domains:
`record_action_audit_event` writes an authenticated action with scope, stable
action/reason symbols, affected resource and attribute when applicable, and
optional non-sensitive numeric before/after values; `action_audit_trail`
provides the maintainer-only query. New sensitive mutation paths should write
only after authorization and validation succeed, and should add tests for actor
attribution, event shape, and maintainer-only access.

## Related: invariants and threat model

The maintainer audit trail on this page records *that* a sensitive mutation
happened and who did it. It does not state what protocol-level guarantee that
mutation is supposed to preserve, or what an adversary could still attempt
around it — those are covered separately:

- [`SECURITY_INVARIANTS.md`](./SECURITY_INVARIANTS.md): the formal invariants
  (token conservation across Treasury/Aid, referral graph acyclicity and
  reward non-dilution, oracle staleness/quorum bounds, access-control's
  admin non-revocability), each tied to the function and error variant that
  enforces it.
- [`THREAT_MODEL.md`](./THREAT_MODEL.md): trust assumptions for admin keys
  and oracle submitters, attack vectors per contract family, and which pause
  mechanism applies where.

## Validation

```bash
cargo test -p shared -p treasury-contract
```
