# Canonical Lifecycle Events (Issue #144)

Every lifecycle-changing operation in the Trellis contracts emits a
**canonical, versioned event** so an indexer can reconstruct state changes
without knowing which contract version produced them or which fields a given
era happened to include.

The canonical envelope lives in `shared::lifecycle_events`.

## Envelope

```text
topics: ("lifecycle", <resource>, <transition>)
data:   LifecycleEvent {
            schema_version: u32,
            resource:       Symbol,
            resource_id:    u64,
            transition:     Symbol,
            from_state:     Symbol,
            to_state:       Symbol,
            actor:          Address,
            timestamp:      u64,
        }
```

| Field | Meaning |
| --- | --- |
| `schema_version` | Payload schema version. Current: `1` (`LIFECYCLE_EVENT_SCHEMA_VERSION`). |
| `resource` | Resource kind: `aid`, `escrow`, `proposal`, or `contract`. |
| `resource_id` | Numeric ID of the record that transitioned (aid ID, escrow ID, proposal ID, record ID). |
| `transition` | Transition name (see inventory below). |
| `from_state` | State before the transition; `none` (`STATE_NONE`) for a creation. |
| `to_state` | State after the transition. |
| `actor` | Address that triggered the transition. |
| `timestamp` | Ledger timestamp of the transition. |

Topics contain only bounded symbols — never addresses or free-form text — so
they are safe, cheap, and stable to filter on.

## Transition inventory

| Resource | Transition | from → to |
| --- | --- | --- |
| `aid` | `created` | `none` → `pending` |
| `aid` | `claimed` | `pending` → `settled` |
| `aid` | `settled` | `pending` → `settled` |
| `aid` | `refunded` | `pending` → `refunded` |
| `escrow` | `created` | `none` → `active` |
| `escrow` | `released` | `active` → `released` |
| `escrow` | `refunded` | `active` → `refunded` |
| `proposal` | `created` | `none` → `pending` |
| `proposal` | `approved` | `pending` → `pending` |
| `proposal` | `executed` | `pending` → `executed` |
| `contract` | `active` | `deactive` → `active` |
| `contract` | `deactive` | `active` → `deactive` |

`Activated` maps to the `active` symbol and `Deactivated` to `deactive`, matching
`shared::lifecycle::ContractRecordState`.

## Emitting

```rust
use shared::lifecycle_events::{
    emit_aid_event, LifecycleTransition, STATE_NONE,
};
use soroban_sdk::symbol_short;

emit_aid_event(
    &env,
    aid_id,
    LifecycleTransition::Created,
    STATE_NONE,
    symbol_short!("pending"),
    &donor,
    env.ledger().timestamp(),
);
```

Lower-level helpers are also available:

- `emit_lifecycle_transition(env, resource, id, transition, from, to, actor, ts)`
  — raw symbol-based emission.
- `emit_resource_transition(env, LifecycleResource, id, LifecycleTransition, from, to, actor, ts)`
  — typed emission.
- `emit_aid_event` / `emit_escrow_event` / `emit_proposal_event` /
  `emit_contract_record_event` — per-resource convenience wrappers.

These events are **additive**. A contract may keep emitting its legacy
per-module events (for example `AID_CREATED`) for backward compatibility while
migrating to the canonical envelope.

## Order guarantee

A workflow emits its transitions in causal order and within a single ledger
they appear in emission order. Tests assert the exact sequence, so a dropped or
reordered event fails the build rather than shipping.

`shared::lifecycle_events::assert_lifecycle_sequence(observed, expected)`
panics on any missing, extra, or reordered event. The `testing` crate wraps it
with environment-level helpers:

```rust
use testing::lifecycle_events::{assert_emitted_lifecycle_sequence, decode_lifecycle_events};

// ... invoke the contract ...
let observed = decode_lifecycle_events(&env);
assert_emitted_lifecycle_sequence(&env, &expected);
```

`decode_lifecycle_events` ignores non-lifecycle events, so a test can mix
legacy, telemetry, and lifecycle emissions and still assert only the canonical
sequence.

## Validation

```bash
# Canonical envelope: content, schema version, resource IDs, ordering.
cargo test -p shared lifecycle_events

# Reusable assertion harness.
cargo test -p testing lifecycle_events
```

The `should_panic` tests in both suites intentionally drop and reorder events
to prove the assertions fail when the event contract is violated.

## Versioning

`LIFECYCLE_EVENT_SCHEMA_VERSION` is stamped on every payload. When the field
set changes:

1. Bump `LIFECYCLE_EVENT_SCHEMA_VERSION`.
2. Keep the previous field semantics readable for at least one release.
3. Add a `changelog/entries.json` entry (impact `compatible` for added fields,
   `breaking` if an existing field changes meaning).
