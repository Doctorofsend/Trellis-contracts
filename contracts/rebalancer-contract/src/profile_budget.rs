//! CI budget-profiling harness for the rebalancer contract (issue #192).
//!
//! Prints the CPU-instruction and memory-byte cost of `rebalance` (the
//! non-dry-run, order-executing path) as a machine-parseable `BUDGET_METRIC`
//! line. See `contracts/aid-contract/src/profile_budget.rs` for the shared
//! design notes and `docs/OBSERVABILITY.md` ("Automated Budget Profiling")
//! for how `scripts/profile-budget.sh` turns these lines into a pass/fail
//! check.
//!
//! As of this writing this module cannot actually run: `rebalancer-contract`
//! does not build at all, in any profile, test or not. `lib.rs` and
//! `logging.rs` call `shared::events::emit_action_executed` with its
//! pre-issue-#151 six-argument signature; that function gained a
//! `correlation_id: &BytesN<32>` parameter (commit 2ff889d / PR #194), and
//! these two call sites were never updated to match. `tests.rs` separately
//! fails to parse (`#[!cfg(test)]` — the `!` needs to be `#![cfg(test)]`) and
//! references a nonexistent `soroban_std` crate (almost certainly meant
//! `soroban_sdk`). All of this is pre-existing on `main`, unrelated to issue
//! #192, and out of scope to fix here — see `docs/OBSERVABILITY.md`
//! ("Automated Budget Profiling") for how `scripts/profile-budget.sh` reports
//! a contract in this state as MISSING rather than failing the whole job.
//! This module is still written and wired in so `rebalancer.rebalance`
//! starts being measured automatically the moment the contract builds again.

#![cfg(test)]

extern crate std;

use crate::{ExecutionStrategy, MultiAssetRebalancer, MultiAssetRebalancerClient, Trade};
use soroban_sdk::{symbol_short, Env, Vec};

/// Host resources consumed by one measured operation.
struct BudgetCost {
    cpu_instructions: u64,
    memory_bytes: u64,
}

/// Runs `f` and returns its result alongside the host resources it alone
/// consumed. Mirrors `treasury-contract::budget_test::measure` (issue #158).
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
fn profile_rebalance() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register_contract(None, MultiAssetRebalancer);
    let client = MultiAssetRebalancerClient::new(&env, &contract_id);

    let mut trades: Vec<Trade> = Vec::new(&env);
    trades.push_back(Trade {
        asset_pair: (symbol_short!("USDC"), symbol_short!("XLM")),
        amount: 1000,
    });

    let (report_out, cost) = measure(&env, || {
        client.rebalance(&trades, &ExecutionStrategy::Balanced, &false)
    });

    assert!(
        report_out.trades_executed + report_out.trades_failed > 0,
        "the profiled rebalance call must process at least one trade"
    );
    report("rebalancer.rebalance", &cost);
}
