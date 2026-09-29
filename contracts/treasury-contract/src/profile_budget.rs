//! CI budget-profiling harness for the treasury contract (issue #192).
//!
//! Prints the CPU-instruction and memory-byte cost of `withdraw` as a
//! machine-parseable `BUDGET_METRIC` line, for `scripts/profile-budget.sh` to
//! collect and compare against `testing/budget-baseline.json`.
//!
//! This module is intentionally separate from `budget_test` (issue #158),
//! which already measures `withdraw` but asserts it against hard-coded,
//! per-contract constants baked into the crate. That suite answers "did this
//! PR make `withdraw` meaningfully more expensive than the commit that wrote
//! the constant." This module answers a related but different question for
//! CI: "compared to `main`, right now, by how much" — using the same
//! `measure` technique but reporting the raw number so the 10% threshold can
//! be tuned in one place (`scripts/profile-budget.sh`) for every profiled
//! contract instead of edited per-crate.
//!
//! As of this writing this module cannot actually run: `cargo build -p
//! treasury-contract` succeeds, but `cargo test -p treasury-contract` does
//! not, because the crate's pre-existing `test.rs` fails to compile (calls
//! `try_schedule_withdraw` with a stale argument list, references a
//! nonexistent `shared::Error::ActionNotYetValid`, and calls `.unwrap()` on a
//! `soroban_sdk::Vec` that has no such method). Since every `#[cfg(test)]`
//! module in a crate is compiled together, that failure currently also blocks
//! `budget_test` (issue #158) from running — pre-existing, unrelated to issue
//! #192, and out of scope to fix here. See `docs/OBSERVABILITY.md`
//! ("Automated Budget Profiling") for how `scripts/profile-budget.sh` reports
//! a contract in this state as MISSING rather than failing the whole job.

#![cfg(test)]

extern crate std;

use crate::{TreasuryContract, TreasuryContractClient};
use soroban_sdk::{symbol_short, testutils::Address as _, Address, Env};

/// Host resources consumed by one measured operation.
struct BudgetCost {
    cpu_instructions: u64,
    memory_bytes: u64,
}

/// Runs `f` and returns its result alongside the host resources it alone
/// consumed. Mirrors `budget_test::measure` in this same crate; duplicated
/// rather than shared so this module stays runnable in isolation (see the
/// module docs above).
fn measure<T, F: FnOnce() -> T>(env: &Env, f: F) -> (T, BudgetCost) {
    env.budget().reset_unlimited();

    let cpu_before = env.budget().cpu_instruction_cost();
    let memory_before = env.budget().memory_bytes_cost();

    let output = f();

    let cost = BudgetCost {
        cpu_instructions: env
            .budget()
            .cpu_instruction_cost()
            .saturating_sub(cpu_before),
        memory_bytes: env
            .budget()
            .memory_bytes_cost()
            .saturating_sub(memory_before),
    };
    (output, cost)
}

/// Prints the `BUDGET_METRIC` line `scripts/profile-budget.sh` scrapes out of
/// `cargo test -- --nocapture` output.
///
/// Leads with a blank line: the test harness prints its own progress line to
/// stdout *without* a trailing newline before the test body runs, so a bare
/// `println!` here would land mid-line instead of starting a fresh line the
/// `^BUDGET_METRIC` parser in `scripts/compare-budget.cjs` can match.
fn report(key: &str, cost: &BudgetCost) {
    std::println!(
        "\nBUDGET_METRIC {key} cpu={} mem={}",
        cost.cpu_instructions,
        cost.memory_bytes
    );
}

#[test]
fn profile_withdraw() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register_contract(None, TreasuryContract);
    let client = TreasuryContractClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    client.initialize(&admin, &1_000);

    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    soroban_sdk::token::StellarAssetClient::new(&env, &token).mint(&admin, &10_000);

    let category = symbol_short!("reserve");
    let recipient = Address::generate(&env);
    client.deposit(&admin, &token, &category, &500);

    let (result, cost) = measure(&env, || {
        client.try_withdraw(&admin, &token, &recipient, &200, &category)
    });

    assert!(result.is_ok(), "the profiled withdraw call must succeed");
    report("treasury.withdraw", &cost);
}
