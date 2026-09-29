# Security invariants

This document states the protocol-level invariants Trellis contracts rely on,
grounded in the actual entry points, storage keys, and error variants that
enforce them. It is a companion to [`AUDIT.md`](./AUDIT.md) (what gets
recorded when state changes) and [`THREAT_MODEL.md`](./THREAT_MODEL.md) (who
is trusted, and what an adversary can still do). None of the contracts listed
here have had a third-party audit — see [`../SECURITY.md`](../SECURITY.md).

Each invariant below is stated formally, names the function(s) that enforce
it, and names the error a violation actually returns today (not a proposed
one). Where the current implementation does not fully close a gap, that is
called out explicitly rather than glossed over — the point of this document
is to be checkable against the code, not to describe an idealized system.

---

## 1. Token conservation — Treasury, Aid, and escrow

### 1.1 Aid escrow

**Invariant.** For every `token`, the aid-contract's on-chain balance of
`token` equals the sum of `amount` over every `AidRecord` with
`status == AidStatus::Pending` and that `token`:

```
token.balance_of(aid_contract) == Σ { record.amount | record.status == Pending, record.token == token }
```

**Enforcement.** `AidContract::create_aid` (`contracts/aid-contract/src/lib.rs`)
writes the `AidRecord` with `status = Pending` *before* calling
`token::Client::transfer(&donor, &contract_address, &amount)` — the record and
the escrowed funds are created together. `AidContract::claim_aid` flips the
record to `Settled` and transfers `record.amount` from the contract to
`record.recipient` in the same call; `AidContract::refund_aid` flips it to
`Refunded` and transfers `record.amount` back to `record.donor`. Both
claim and refund are guarded to run at most once per record: `claim_aid`
rejects `Settled`/`Refunded` records with `AidError::AlreadyClaimed`, and
`refund_aid` separately rejects `Settled` with `AidError::AlreadyClaimed` and
`Refunded` with `AidError::AlreadyRefunded`. Because every status transition
(`Pending → Settled`, `Pending → Refunded`) is paired 1:1 with exactly one
token transfer of exactly `record.amount`, and a record can only transition
once, the sum above holds by construction. `AidError::NotExpiredYet` blocks a
refund from firing on the same funds a still-live claim window could also
settle.

### 1.2 Treasury category balances

**Invariant (as designed).** For every `(token, category)` pair, the stored
counter equals the funds earmarked for that category:

```
category_balance(token, category) == Σ deposits(token, category) − Σ withdrawals(token, category)
```

`TreasuryContract::category_balance(token, category)` reads the
`(BALANCE, token, category)` instance-storage entry (`contracts/treasury-contract/src/lib.rs`).
`deposit` increments it and immediately pulls `amount` of `token` from the
caller via `token::Client::transfer(&caller, &contract_address, &amount)` in
the same call, so a deposit's bookkeeping and its token movement are atomic.

**Enforcement caveat — read this before relying on category balances for
reconciliation.** `withdraw`, `execute_scheduled_withdraw`,
`emergency_withdraw`, and `distribute_reward` all decrement the relevant
`(BALANCE, token, category)` counter and emit the corresponding withdrawal /
`CommissionPaid` event, but **none of them call `token::Client::transfer`** —
grep the file: the only `token::Client::new(...)` call in
`treasury-contract/src/lib.rs` is inside `deposit`. In the current
implementation, category balances are therefore a pure internal ledger of
*intended* outflows; they are not proof that `token` actually left the
contract on any withdrawal path. The practical consequence is:

```
token.balance_of(treasury_contract) >= Σ_categories category_balance(token, category)
```

is not guaranteed to hold as an equality the way it does for `deposit`/Aid
escrow — the real on-chain balance can only be *higher* than the sum of
booked categories (funds sent in via `deposit` or direct transfer that were
never booked out), never verified lower. Operators reconciling treasury state
should compare `category_balance` sums against the token contract's actual
`balance()` for the treasury address as a routine health check, and should
not assume a successful `withdraw`/`distribute_reward` call moved tokens
without independently verifying the recipient's balance. This is a
documentation-visible gap, not a hidden one: it is the single most
security-relevant fact in this file for anyone reasoning about "does the
treasury still hold what it says it holds."

**Guards that do hold regardless of the above:** every debiting path
validates `amount > 0` (`Error::InvalidArgument`), `amount <= withdrawal
limit` (`Error::WithdrawalLimitExceeded`, checked against `MAX_WD`), and
`amount <= category balance` (`Error::InsufficientBalance`) before mutating
storage, and `withdraw`/`execute_scheduled_withdraw` additionally consume a
quota via `shared::quota::check_and_consume`. `emergency_withdraw` further
requires `shared::storage::is_paused(&env)` to be `true` — it is unreachable
while the treasury is live — and only ever debits the `reserve` category.
`distribute_reward` requires the direct caller to be the address stored under
`REFERRAL_CONTRACT` (set via `set_referral_contract`, admin-only) with the
`ServiceActor` role, via `auth::require_permission(.., Permission::ServiceOperation)`;
an unset or mismatched caller gets `Error::Unauthorized`.

---

## 2. Referral graph acyclicity and non-dilution bounds

Source: `contracts/referral-contract/src/lib.rs`.

### 2.1 Acyclicity

**Invariant.** Following `Referrer(wallet) -> referrer` edges from any wallet
never revisits a wallet already on the path, for at least
`MAX_SUPPORTED_TIERS` (10) hops.

**Enforcement.** Both `set_referrer` (admin-only) and `register`
(self-service, `wallet.require_auth()`) call `would_create_cycle(env,
referred_wallet, referrer)` before writing the `Referrer` edge. That function
walks upward from `referrer` through existing `Referrer` edges for up to
`MAX_SUPPORTED_TIERS` hops, returning `true` (rejected with
`Error::InvalidArgument`) if `referred_wallet` appears anywhere on that walk.
Self-referral (`wallet == referrer`) is rejected by the same error before the
walk even starts. `register` additionally requires the target `referrer` to
already have its own referrer edge (`read_referrer(&env, &referrer).is_none()`
→ `Error::InvalidArgument`), which prevents a caller from bootstrapping a
fresh, disconnected sub-graph outside admin control.

**Known bound.** `would_create_cycle`'s walk is capped at
`MAX_SUPPORTED_TIERS` hops (`depth < MAX_SUPPORTED_TIERS`); a cycle that
closes strictly beyond that depth would not be detected by this check alone.
This is bounded in practice by a second, independent invariant: `accrue`'s
own traversal (§2.2) is capped at `max_tiers` (validated ≤
`MAX_SUPPORTED_TIERS` by `set_tier_config`), so reward accrual can never walk
far enough to enter a cycle even if one existed past the checked depth. The
acyclicity check is defense-in-depth for graph *correctness* (e.g. off-chain
consumers walking the full chain); it is not load-bearing for fund safety,
which rests on the bounded accrual walk instead.

### 2.2 Tier commission non-dilution

**Invariant.** For a single `accrue(base_amount)` call, total commission
credited across all tiers never exceeds `base_amount`:

```
Σ_tier credited(tier) <= base_amount
```

**Enforcement.** `set_tier_config`'s validator, `validate_tier_config`,
requires every individual `tier_bps[i]` to be in `MIN_TIER_BPS..=MAX_TIER_BPS`
(`0..=10_000`, i.e. 0–100%) *and* sums all configured tiers' bps into
`total_bps`, rejecting the whole configuration with `Error::InvalidArgument`
if `total_bps > MAX_TIER_BPS` (10,000 = 100%). Since `accrue` computes each
tier's commission as `shared::math::bps_of(base_amount, tier_bps)`, and the
sum of all `tier_bps` values is capped at 10,000, the sum of all tiers'
commissions is capped at `base_amount` by construction — no configuration can
be accepted that would pay out more than the base amount across the whole
referral chain.

### 2.3 Lifetime reward cap

**Invariant.** For every referrer, cumulative credited commission never
exceeds the configured `reward_cap`:

```
lifetime_accrued(referrer) <= reward_cap  (for all time)
```

**Enforcement.** `credit_referrer` reads `lifetime_accrued(referrer)` before
crediting, short-circuits to `0` credited if it is already `>= reward_cap`,
and otherwise clamps `credited = min(commission, reward_cap -
lifetime_accrued)`. Both `accrued_balance` and `lifetime_accrued` are updated
with `shared::math::safe_add`, which returns `Error::Overflow` rather than
wrapping. `accrue` itself is admin/service-gated
(`require_admin` → `Permission::ReferralConfiguration`) — an end wallet
cannot self-trigger accrual, so Sybil-registering many wallets under oneself
cannot mint rewards without a privileged caller choosing to call `accrue`
against attacker-controlled traffic.

---

## 3. Oracle staleness and quorum bounds

Source: `contracts/oracle-contract/src/lib.rs`, `errors.rs`.

**What the contract actually enforces today:** replay protection, a future-skew
bound, a staleness window, decimals consistency, and a minimum submission
quorum. **It does not enforce a maximum deviation between a submitted price
and the current median** — there is no such check in `submit_price` or
`aggregate_feed_latest`. Stating this precisely (rather than inventing a
basis-point threshold that isn't in the code) is intentional; see
[`THREAT_MODEL.md`](./THREAT_MODEL.md) for the residual risk this leaves.

### 3.1 Submitter authorization

**Invariant.** Only addresses with an active submitter registration may post
prices. `OracleContract::submit_price` requires `submitter.require_auth()`
and rejects with `OracleError::SubmitterNotAuthorized` (500) unless
`storage::is_submitter_active(&env, &submitter)` is `true`. Submitters are
added by `register_submitter` and removed by `deactivate_submitter`, both
gated by `shared::auth::require_admin`, which return
`OracleError::Unauthorized` (508) for a non-admin caller.

### 3.2 Replay protection

**Invariant.** Each `(submitter, feed_id)` pair's accepted nonce is strictly
increasing by exactly 1. `submit_price` reads
`storage::get_nonce(&env, &submitter, &feed_id)` as `expected_nonce` and
rejects with `OracleError::DuplicateSubmission` (501) unless `nonce ==
expected_nonce + 1`; on success it writes the new nonce with
`storage::set_nonce`. A submission cannot be replayed, reordered, or skipped
without the submitter incrementing sequentially.

### 3.3 Staleness and future-skew bounds

**Invariant.** A submission's declared `timestamp` must satisfy:

```
current_time - staleness_window <= timestamp <= current_time + MAX_FUTURE_SKEW_SECS
```

where `MAX_FUTURE_SKEW_SECS = 60` (a fixed contract constant) and
`staleness_window` is admin-configurable via `set_staleness_window`
(`shared::auth::require_admin`-gated). `submit_price` rejects
`timestamp > current_time.saturating_add(MAX_FUTURE_SKEW_SECS)` with
`OracleError::SubmissionFromFuture`, and rejects
`current_time > timestamp.checked_add(staleness_window)` (or an overflowing
add) with `OracleError::SubmissionStale` (504). The same window is
re-checked per-submission inside `aggregate_feed_latest` when building the
active set that feeds the median, so a submission that was fresh when
submitted but has since aged past `staleness_window` is silently excluded
from the aggregate rather than corrupting it.

### 3.4 Quorum-gated aggregation

**Invariant.** `get_latest_price` never reflects fewer than
`MIN_PRICE_QUORUM` (2) independent, active, non-stale, decimals-consistent
submissions for that feed.

**Enforcement.** `aggregate_feed_latest` builds `prices` only from active
submissions whose submitter is still `is_submitter_active`, whose
`decimals` match the first-seen value for the batch (mismatched-decimals
submissions are dropped from that round, not error'd), and whose timestamp is
within the window in §3.3. If `prices.len() < MIN_PRICE_QUORUM`, the function
returns `None` and `storage::set_feed_latest` is not called — the previous
`FeedLatest` (if any) is left untouched rather than being overwritten by an
under-quorum read. When quorum is met, the aggregate is the sorted median
(`sort_prices` + midpoint average on even counts), so a single submitter can
never singlehandedly move `get_latest_price` — but see §3 above: with the
minimum quorum of 2, two colluding or compromised submitters are sufficient,
and no on-chain deviation bound limits how far from prior consensus their
submissions can move the reported price.

---

## 4. Access-control privilege hierarchy and admin non-revocability

Source: `contracts/access-control/src/lib.rs`, `permissions.rs`.

### 4.1 Role hierarchy is acyclic and grants flow downward only

**Invariant.** `RoleParent(role) -> parent` edges form a forest (no cycles);
holding `parent` implies holding every descendant role, never the reverse.

**Enforcement.** `set_role_parent` rejects `role == parent` with
`AccessControlError::SelfReference` and rejects the edge if `parent` already
has `role` as an ancestor (`has_ancestor(env, parent, role)`) with
`AccessControlError::CycleDetected` — checked *before* the edge is written, so
an invalid edge is never persisted. `has_role_recursive` (used by the public
`has_role` and the whole permission matrix) checks direct membership first,
then walks `RoleParent` upward; it has no path that walks downward from a
role to a role that named it as parent, so grants only propagate from a
parent role to its declared children, never sideways or up.

### 4.2 Every privileged action is authorized by exactly one matrix row

**Invariant.** Every access-control entry point that mutates protected state
calls `require_action(&env, &caller, &Action::…)` exactly once, and
`policy_for(action)` is the single source of truth for the
capability/role/scope that action requires (`permissions.rs`). There is no
second, bespoke authorization path for these entry points.

**Enforcement / failure modes.** `has_action_permission` evaluates scope:
- **Global** (`ManageMaintainers`, `ConfigureRoleRegistry`, `AssignRoles`,
  `ReadAuditTrail`, `TransferOwnership`, `CancelOwnershipTransfer`,
  `PauseScope`) requires `has_maintainer_authority` — this contract's own
  `Admins` map or the shared capability model. Failure:
  `AccessControlError::NotAdmin` (206).
- **Role-scoped** (`InviteMember(role)`) is satisfied by a maintainer *or* a
  holder of `role` — a holder of a different role is rejected with
  `AccessControlError::RoleEscalation` (211), proven by
  `role_holder_may_invite_only_into_their_own_role` in `permissions.rs`'s own
  test module.
- **Owner-scoped** (`CancelInvitation(inviter)`) is satisfied by a maintainer
  *or* the original inviter — a peer holding the same role is rejected with
  the same `RoleEscalation`, proven by
  `inviter_may_cancel_only_their_own_invitation`.

`create_invitation` additionally rate-limits each caller to 10 invitations
per rolling hour (`InviteCount`/`LastInviteTime`), returning
`AccessControlError::RateLimitExceeded` (212) past that bound, independent of
whether the caller is otherwise authorized.

### 4.3 The super-admin cannot be silently removed

**Invariant.** The address stored at `DataKey::SuperAdmin` can never be
deleted from the `Admins` map by `remove_admin`; it can only be *replaced* by
an explicit, two-party, time-boxed transfer that the new owner must actively
accept.

**Enforcement.** `remove_admin` compares `target == super_admin_addr` and
returns `AccessControlError::CannotRemoveSuperAdmin` (207) unconditionally —
this check runs before the idempotent not-an-admin short-circuit, so it
cannot be bypassed by racing state. The only way to change who holds the
`super` role is the ownership-transfer flow: `transfer_ownership` (caller
must *be* the current super-admin, checked against `SuperAdmin` storage, not
just hold `Admin`) writes a `PendingOwnershipTransfer{current_owner,
target_owner, expiry}` and rejects a self-transfer with
`AccessControlError::SelfReference` and a past/now expiry with
`AccessControlError::InvalidExpiry`; a second proposal while one is still
live is rejected with `AccessControlError::TransferAlreadyPending`.
`accept_ownership` requires the caller to *be* `pending.target_owner`
(`AccessControlError::NotPendingOwner` otherwise) and to call before
`pending.expiry` (`AccessControlError::TransferExpired` otherwise, which also
clears the pending record so it cannot be accepted late). Only on acceptance
does the contract update `SuperAdmin`, grant `super` to the new owner, and
revoke it from the old owner, atomically. `cancel_ownership_transfer` lets
the *current* super-admin abort a pending transfer
(`AccessControlError::NoPendingTransfer` if none exists). No code path sets
`SuperAdmin` any other way after `initialize`, which itself can only run once
(`AccessControlError::AlreadyInitialized`).

**Non-invariant worth flagging.** Regular (non-super) admins are not subject
to any of this: any existing admin can `add_admin`/`remove_admin` any other
non-super admin via the `ManageMaintainers` global action — this is an *any
admin* (OR) authority model, not a threshold. A single compromised
non-super-admin key is sufficient to add further admin accounts or remove
peers. See [`THREAT_MODEL.md`](./THREAT_MODEL.md) §1 for the operational
implications and how this contrasts with `governance-contract`'s M-of-N
proposal flow.

---

## Validation

```bash
cargo test -p access-control
cargo test -p treasury-contract
cargo test -p aid-contract
cargo test -p referral-contract
cargo test -p oracle-contract
```

The referral-contract suite includes explicit cycle-rejection
(`rejects_invalid_admin_config_and_cycles`, `register_rejects_*`) and
cap-enforcement tests (`enforces_lifetime_reward_cap_per_referrer`,
`accrual_math_rejects_overflow`). The access-control `permissions` module's
own test suite (`every_action_maps_to_one_capability_and_scope`,
`outsider_is_denied_every_global_action`,
`role_holder_may_invite_only_into_their_own_role`,
`inviter_may_cancel_only_their_own_invitation`) directly exercises §4.2.
