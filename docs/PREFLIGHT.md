# Operation Preflight

`shared::preflight` is the deterministic, pre-submission check for high-risk
Trellis operations. It runs the existing business-rule, policy and safe-message
primitives together and returns a single actionable verdict the caller can act
on **before** signing or submitting a transaction.

- Module: [`shared/src/preflight.rs`](../shared/src/preflight.rs)
- Re-exports: `shared::{preflight, require_preflight, requires_preflight, PreflightInput, PreflightReport, ...}`

## Why

The workspace already had the raw materials:

- `shared::policy` evaluates maintainer-tunable business rules, but returns a
  binary allowed/denied decision with no user guidance.
- `shared::semantic` validates individual values in isolation.
- `shared::error_taxonomy` renders safe messages, but only *after* a call has
  already failed.
- `testing::simulation` exercises scenarios, but lives in the test harness.

Nothing combined them into a verdict a caller can run first. `preflight` does,
without adding a second copy of any rule: it calls `policy::evaluate` for the
business rules and reuses `ErrorCategory` and the shared `Error` codes so
contracts, clients, and tests stay on one taxonomy.

## Statuses

Every check resolves to one of three states, and the report's overall status is
`Blocked` if any check is blocked, otherwise `Warning` if any check warns,
otherwise `Ready`.

| Status | Meaning | Caller action |
| --- | --- | --- |
| `Ready` | No findings. | Submit. |
| `Warning` | Submittable, but the caller should read the explanation and remediation. | Acknowledge, or fix the flagged risk first. |
| `Blocked` | Known-invalid; submission will fail or is unsafe. | Do not submit — follow the remediation and re-run preflight. |

`PreflightReport::primary_code` is the first `Blocked` code, or the first
`Warning` code when nothing is blocked. `checks` keeps every finding in a fixed
evaluation order so callers can disclose warnings even alongside a block.

## Which operations need preflight

`PreflightOperation::risk_level` classifies each operation; `requires_preflight`
is `true` for anything above `RiskLevel::Standard`.

| Operation | Risk | Why |
| --- | --- | --- |
| `TreasuryWithdraw` | High | Moves held funds. |
| `EscrowRelease` / `EscrowRefund` | High | Irreversible settlement transfer. |
| `UpgradeExecute` | High | Replaces contract code. |
| `MigrationResume` | High | Mutates persisted storage layout. |
| `RebalanceExecute` | High | Moves funds between strategies. |
| `BatchInvoke` | Elevated | Multi-operation, harder to reason about. |
| `AidSettle` | Elevated | Settlement-adjacent. |
| `Transfer` | Standard | Ordinary validated path. |

## Checks

Checks run in this fixed order; the table lists the code emitted, its status,
and the reused [`ErrorCategory`].

| Check | Code | Status | Category |
| --- | --- | --- | --- |
| Contract paused | `ContractPaused` | Blocked | Conflict |
| Expected state version ≠ observed | `StaleState` | Blocked | Conflict |
| Expiry ledger reached | `Expired` | Blocked | Conflict |
| Expiry within `EXPIRY_WARNING_WINDOW_LEDGERS` | `ExpirySoon` | Warning | Conflict |
| Policy configuration invalid | `InvalidConfig` | Blocked | Configuration |
| Amount below/above policy range | `AmountBelowMinimum` / `AmountAboveMaximum` | Blocked | Validation |
| Actor tier not allowed | `TierNotAllowed` | Blocked | Authorization |
| Region restricted | `RegionRestricted` | Blocked | Authorization |
| Daily operation cap reached | `DailyLimitReached` | Blocked | Conflict |
| One daily slot remains | `ApproachingDailyLimit` | Warning | Conflict |
| Amount ≥ `HIGH_VALUE_WARNING_BPS` of the max | `HighValue` | Warning | Settlement |

Every `Warning` and `Blocked` check carries a `message` (user-safe, no internal
state) and a `remediation: Option<String>` describing the next step.

## Usage

`preflight` is pure: the caller supplies every ledger, time, and state fact, and
the same input always yields an equal report. It never reads the ledger,
storage, or a clock.

```rust
use shared::preflight::{preflight, require_preflight, PreflightInput, PreflightOperation};

// Typed facts assembled by the caller — deterministic, no env reads inside.
let input = PreflightInput {
    operation: PreflightOperation::TreasuryWithdraw,
    amount,
    actor_tier,
    daily_ops,
    region_allowed,
    config,
    paused,
    expected_state_version, // what the prepared tx was based on
    observed_state_version, // what storage says now
    current_ledger: env.ledger().sequence(),
    expiry_ledger,
};

// Option A — inspect the report and disclose warnings yourself.
let report = preflight(&env, &input);
if report.is_blocked() {
    // report.blocking_checks()[0].message / .remediation
}

// Option B — guard an entry point; blocks map to the shared Error codes.
let report = require_preflight(&env, &input)?; // Err on Blocked, Ok(report) on Ready/Warning
```

`require_preflight` returns the report (rather than discarding warnings) so a
contract can feed the findings into `shared::disclosure` before submission.

### Error mapping

`require_preflight` mirrors `policy::require_policy` so entry points keep
returning stable, user-safe errors:

| Primary block | `Error` |
| --- | --- |
| `ContractPaused` | `ContractPaused` |
| `StaleState` | `StaleData` |
| `Expired` | `Expired` |
| `AmountBelowMinimum` / `AmountAboveMaximum` | `InvalidAmount` |
| `TierNotAllowed` / `RegionRestricted` | `Unauthorized` |
| `DailyLimitReached` | `WithdrawalLimitExceeded` |
| `InvalidConfig` | `ConfigInvalid` |

## Determinism

Preflight is a pure function of `PreflightInput`. The regression suite
(`shared/src/preflight.rs`, `preflight::tests`) asserts that equal inputs
produce equal reports, and covers a ready operation, both warnings, a policy
block, a stale-state block, a passed expiry, and an invalid configuration. It
runs offline with no fixtures:

```sh
cargo test -p shared --lib preflight::
```

## Design tradeoffs

- **Pure function over on-chain simulation.** A Soroban `Env` cannot re-run a
  transaction against speculative state before submission. Passing the relevant
  facts in keeps preflight deterministic, testable, and cheap, at the cost of
  the caller having to read the observed state version. Contracts already read
  state at their entry point, so the fields are available where the guard is
  called.
- **Reuse, not replacement.** Preflight delegates all business-limit logic to
  `policy::evaluate` so a maintainer tuning `PolicyConfig` changes both the
  guard and the preflight verdict in one place.
- **Three statuses, not two.** `policy` deliberately stays binary; preflight
  layers the advisory `Warning` state on top and never downgrades a block.

## Migration and configuration

None. This adds a new module and re-exports; no existing contract interface,
storage key, error discriminant, event, or `PolicyConfig` field changes.
Adopting a guard in a given contract entry point is an independent follow-up.
