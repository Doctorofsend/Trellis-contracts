# Observability

Trellis emits structured **telemetry events** from every critical contract
path. Off-chain indexers turn those events into latency, failure-rate, and
business-conversion dashboards without reading contract storage. This document
defines the payload, the metric names, and ready-to-use dashboard queries.

See also [`DIAGNOSTICS.md`](./DIAGNOSTICS.md) for the local pre-flight script
and [`RUNBOOK.md`](./RUNBOOK.md) for operational procedures.

For snapshot-based contract state verification helpers used in high-risk test
suites, see [`CONTRACT_STATE_SNAPSHOTS.md`](./CONTRACT_STATE_SNAPSHOTS.md).

## Operational Health Indicators

The `shared::dashboard` module aggregates key indicators to track operational health:
- **Unresolved Failures & Stale Jobs**: Derived from background worker dead letters (failed beyond retry limits).
- **Reconciliation Drift**: Derived from the reconciliation module, highlighting mismatch between ledger, off-chain DB, and user caches.
- **User-Impacting Incidents**: Derived from dependency health checks where critical services are marked degraded or down.

These can be fetched programmatically via the `generate_dashboard` method, which redacts sensitive information such as specific user addresses.

## Event contract

Every telemetry event is published with:

- **Topics:** `("tel", operation)` — filter on the first topic to subscribe to
  telemetry only.
- **Data:** a `TelemetryEvent` with a fixed field set.

| Field | Type | Meaning |
| --- | --- | --- |
| `operation` | `Symbol` | Low-cardinality operation name (see the table below). |
| `actor` | `ActorType` | `User`, `Admin`, `Contract`, `Worker`, or `System`. |
| `result` | `TelemetryResult` | `Success` or `Failure`. |
| `latency_ledgers` | `u32` | Ledgers elapsed between start and finish. `0` for single-ledger operations. |
| `correlation_id` | `u64` | Caller-supplied workflow id, or the ledger sequence when the caller supplies none. |

### Core operations

| Operation | Symbol | Instrumented path |
| --- | --- | --- |
| Payment transfer | `pay_xfr` | `shared::payments::safe_transfer` |
| Escrow creation | `esc_crt` | `shared::payments::create_escrow` |
| Escrow release | `esc_rel` | `shared::payments::release_escrow` |
| Quota consumption | `quota_csm` | `shared::quota::check_and_consume` |
| Strategy rebalance | `rebal` | `contracts/rebalancer-contract::rebalance` |

`shared::telemetry::CORE_OPERATIONS` lists these in code, and
`shared::test_telemetry` asserts that each emits a payload carrying all five
structured fields.

### Sensitive values

Telemetry deliberately carries **no addresses, amounts, balances, tokens,
secrets, or free-form text**. Failures are described by the operation name plus
the correlation id; the contract error is still returned to the caller and, for
payment paths, published through the existing typed events. Keeping telemetry
low-cardinality and value-free also keeps it cheap to index.

### Emitting from a new path

```rust
use shared::telemetry::{ActorType, TelemetryTimer, OP_REBALANCE};

let timer = TelemetryTimer::start_here(&env, OP_REBALANCE, ActorType::Contract);
// ... do the work ...
if worked {
    timer.succeed(&env);
} else {
    timer.fail(&env);
}
```

For single-ledger operations where no timer is needed, use
`emit_success(&env, OP, actor, 0)` / `emit_failure(&env, OP, actor, 0)`.

## Metric names

| Metric | Type | Labels | Definition |
| --- | --- | --- | --- |
| `trellis_operation_total` | counter | `operation`, `actor`, `result` | Telemetry payloads observed. |
| `trellis_operation_success_total` | counter | `operation` | Payloads with `result = Success`. |
| `trellis_operation_failure_total` | counter | `operation` | Payloads with `result = Failure`. |
| `trellis_operation_failure_ratio` | gauge | `operation` | `failure_total / total`. |
| `trellis_operation_latency_ledgers` | histogram | `operation` | Distribution of `latency_ledgers`. |
| `trellis_operation_throughput_total` | counter | `operation` | Same source as `trellis_operation_total`; used for rate panels. |
| `trellis_conversion_total` | counter | `funnel_step` | Business-critical conversions (`esc_crt`, `esc_rel`, `pay_xfr`). |

Map one indexer row per telemetry event:

```text
trellis_operation_total{operation="esc_crt",actor="user",result="success"} 1
```

## Dashboard queries

The queries below assume an indexed table `soroban_events` with one row per
event:

```sql
-- ledger_closed_at TIMESTAMPTZ
-- contract_id      TEXT
-- topic_0          TEXT   -- "tel"
-- operation        TEXT   -- decoded from the payload
-- actor            TEXT
-- result           TEXT   -- "success" | "failure"
-- latency_ledgers  INTEGER
-- correlation_id   BIGINT
```

### Failure rate by operation (last 24h)

```sql
SELECT operation,
       COUNT(*) FILTER (WHERE result = 'failure') AS failures,
       COUNT(*)                                   AS total,
       ROUND(
         COUNT(*) FILTER (WHERE result = 'failure')::numeric
         / NULLIF(COUNT(*), 0), 4
       ) AS failure_ratio
FROM soroban_events
WHERE topic_0 = 'tel'
  AND ledger_closed_at > now() - interval '24 hours'
GROUP BY operation
ORDER BY failure_ratio DESC;
```

### Latency percentiles by operation (last 24h)

```sql
SELECT operation,
       percentile_cont(0.50) WITHIN GROUP (ORDER BY latency_ledgers) AS p50,
       percentile_cont(0.95) WITHIN GROUP (ORDER BY latency_ledgers) AS p95,
       MAX(latency_ledgers)                                          AS worst
FROM soroban_events
WHERE topic_0 = 'tel'
  AND result = 'success'
  AND ledger_closed_at > now() - interval '24 hours'
GROUP BY operation
ORDER BY p95 DESC;
```

### Business conversion funnel (escrow lifecycle, hourly)

```sql
SELECT date_trunc('hour', ledger_closed_at) AS bucket,
       COUNT(*) FILTER (WHERE operation = 'esc_crt' AND result = 'success') AS created,
       COUNT(*) FILTER (WHERE operation = 'esc_rel' AND result = 'success') AS released,
       ROUND(
         COUNT(*) FILTER (WHERE operation = 'esc_rel' AND result = 'success')::numeric
         / NULLIF(COUNT(*) FILTER (WHERE operation = 'esc_crt' AND result = 'success'), 0), 4
       ) AS release_rate
FROM soroban_events
WHERE topic_0 = 'tel'
  AND ledger_closed_at > now() - interval '7 days'
GROUP BY bucket
ORDER BY bucket;
```

### Trace one workflow by correlation id

```sql
SELECT ledger_closed_at, operation, actor, result, latency_ledgers
FROM soroban_events
WHERE correlation_id = 4242
ORDER BY ledger_closed_at;
```

### Where users fail

```sql
SELECT operation, actor, COUNT(*) AS failures
FROM soroban_events
WHERE topic_0 = 'tel'
  AND result = 'failure'
  AND ledger_closed_at > now() - interval '24 hours'
GROUP BY operation, actor
ORDER BY failures DESC;
```

## Subscribing to the raw stream

```bash
# Stellar RPC: pull telemetry events for a contract from a starting ledger
soroban events \
  --id <CONTRACT_ID> \
  --start-ledger <LEDGER> \
  --network testnet \
  --topic-filter-1 "tel"
```

Indexers should key on `correlation_id` to join telemetry to the contract's
typed events (for example `pay_esc_c` / `pay_esc_r`) when a human-auditable
trail is needed.

## Contract state snapshots

Contract test suites use the snapshot helpers documented in
[`CONTRACT_STATE_SNAPSHOTS.md`](./CONTRACT_STATE_SNAPSHOTS.md) to capture
deterministic before/after state and assert expected deltas for balances,
ownership, status, and metadata. Unexpected deltas fail tests with a
structured diff of the observed changes.

## Automated budget profiling

Telemetry and snapshots tell you *what* happened to a call. This section is
about *what it cost*: the Soroban host's CPU-instruction and memory-byte
meters, which are the same quantities the network charges for at runtime.
CI profiles these on every pull request so a change that quietly makes a
critical path more expensive is caught before merge, not after a mainnet
budget-exceeded failure.

### What is profiled

The `profile-budget` CI job (`.github/workflows/ci.yml`) measures one
representative call per contract via `Env::budget()`, the same host API
`contracts/treasury-contract/src/budget_test.rs` uses for its per-PR
regression assertions (issue #158):

| Contract | Call | Harness |
| --- | --- | --- |
| `aid-contract` | `create_aid` | `contracts/aid-contract/src/profile_budget.rs` |
| `aid-contract` | `claim_aid` | `contracts/aid-contract/src/profile_budget.rs` |
| `treasury-contract` | `withdraw` | `contracts/treasury-contract/src/profile_budget.rs` |
| `referral-contract` | `register` | `contracts/referral-contract/src/profile_budget.rs` |
| `rebalancer-contract` | `rebalance` | `contracts/rebalancer-contract/src/profile_budget.rs` |

Each harness resets the budget, calls the contract once through a realistic
fixture, and prints the delta as a machine-readable line:

```text
BUDGET_METRIC aid.create_aid cpu=123456 mem=7890
```

`scripts/profile-budget.sh` runs `cargo test -p <contract> --lib
profile_budget -- --nocapture` for each contract above, collects those
lines, and hands them to `scripts/compare-budget.cjs`.

This is a deliberately different mechanism from `budget_test.rs`'s
`assert_within`: that suite pins hard-coded, per-contract limits inside the
crate and asks "did this change make the call meaningfully worse than the
commit that set the constant." The CI job here asks a cheaper, more general
question -- "compared to the last-recorded baseline, by how much, right
now" -- for every profiled contract from one place, without needing a new
hard-coded constant per call.

### Baseline and threshold

Measured costs are compared against the checked-in baseline at
`testing/budget-baseline.json` (`{ "metrics": { "<contract>.<call>": {
"cpu_instructions": N, "memory_bytes": M }, ... } }`). A call fails the job
when either its CPU instructions or its memory bytes increase by more than
**10%** relative to the baseline. That threshold is deliberately loose, for
the same reason `budget_test.rs`'s `HEADROOM_PERCENT` is: a `soroban-sdk`
patch bump can shift the host's cost model by a few percent, and that alone
should not fail an unrelated PR. A real regression is typically measured in
multiples, not single-digit percentages.

The threshold is tunable without editing the workflow: set the repository
variable `BUDGET_REGRESSION_THRESHOLD_PERCENT` (Settings → Secrets and
variables → Actions → Variables), or pass `--threshold <percent>` when
running `scripts/profile-budget.sh` locally. It defaults to `10`.

### Updating the baseline

When a measured increase is intentional (a new guard, a richer event
payload, an added storage read), regenerate the baseline and explain why in
the PR:

```bash
./scripts/profile-budget.sh --update-baseline
git add testing/budget-baseline.json
```

### Reading the job output

`profile-budget` writes a markdown comparison table to the job's
`$GITHUB_STEP_SUMMARY` (visible on the job's summary page, no PR comment
needed) with one row per profiled call:

- **PASS** — within the threshold.
- **FAIL** — CPU or memory regressed beyond the threshold; the job exits
  non-zero.
- **NEW** — measured this run but absent from the baseline (a call profiled
  for the first time); does not fail the job.
- **MISSING** — present in the baseline but not measured this run, because
  that contract's `profile_budget` tests did not build or run. This is
  reported as a warning, not a job failure, so an unrelated, pre-existing
  problem in one contract's test suite cannot silently block every other
  contract's profiling.

### Running it locally

```bash
./scripts/profile-budget.sh                        # measure and compare
./scripts/profile-budget.sh --threshold 25          # loosen the gate for one run
./scripts/profile-budget.sh --update-baseline       # regenerate the baseline file
```

Or, to inspect a single contract's raw measurement without the comparison
step:

```bash
cargo test -p aid-contract --lib profile_budget -- --nocapture --test-threads=1
```
