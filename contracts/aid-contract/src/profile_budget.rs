//! CI budget-profiling harness for the aid contract (issue #192).
//!
//! Prints the CPU-instruction and memory-byte cost of `create_aid` and
//! `claim_aid` — the contract's two hottest entry points — as a
//! machine-parseable `BUDGET_METRIC` line. `scripts/profile-budget.sh` runs
//! these tests with `--nocapture`, collects the lines across every profiled
//! contract, and compares them against the checked-in
//! `testing/budget-baseline.json`. See `docs/OBSERVABILITY.md` ("Automated
//! Budget Profiling") for how the comparison and the failure threshold work.
//!
//! Deliberately does not assert against a threshold itself: unlike
//! `treasury-contract::budget_test` (issue #158), which pins per-contract
//! regression limits inside the crate, this module only measures and reports.
//! The pass/fail decision lives in CI so the 10% default threshold can be
//! tuned in one place for every profiled contract without touching contract
//! source.

#![cfg(test)]

extern crate std;

use crate::{AidContract, AidContractClient};
use soroban_sdk::{testutils::Address as _, token, Address, Env};

/// Host resources consumed by one measured operation.
struct BudgetCost {
    cpu_instructions: u64,
    memory_bytes: u64,
}

/// Runs `f` and returns its result alongside the host resources it alone
/// consumed. Mirrors `treasury-contract::budget_test::measure` (issue #158);
/// duplicated here per that module's own scope note rather than lifted into
/// `shared`, which would need a new manifest-level feature.
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
/// Leads with a blank line: the test harness prints its own progress line
/// (`test profile_budget::profile_create_aid ... `) to stdout *without* a
/// trailing newline before the test body runs, so a bare `println!` here
/// would land mid-line instead of starting a fresh line the `^BUDGET_METRIC`
/// parser in `scripts/compare-budget.cjs` can match.
fn report(key: &str, cost: &BudgetCost) {
    std::println!(
        "\nBUDGET_METRIC {key} cpu={} mem={}",
        cost.cpu_instructions,
        cost.memory_bytes
    );
}

/// Registers the aid contract plus a stellar-asset token and funds the donor.
fn setup(env: &Env) -> (AidContractClient<'static>, Address, Address) {
    let admin = Address::generate(env);
    let donor = Address::generate(env);
    let recipient = Address::generate(env);

    let token_addr = env.register_stellar_asset_contract(admin.clone());
    token::StellarAssetClient::new(env, &token_addr).mint(&donor, &1_000_000);

    let contract_id = env.register_contract(None, AidContract);
    let client = AidContractClient::new(env, &contract_id);
    let treasury = Address::generate(env);
    client.initialize(&admin, &treasury, &token_addr, &3600);

    (client, donor, recipient)
}

#[test]
fn profile_create_aid() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, donor, recipient) = setup(&env);
    let expiry = env.ledger().sequence() + 10_000;

    let (aid_id, cost) = measure(&env, || {
        client.create_aid(&donor, &recipient, &100, &expiry, &None)
    });

    // Sanity check so a broken fixture fails loudly instead of reporting a
    // cost for a call that never ran.
    assert!(client.get_aid(&aid_id).is_some());
    report("aid.create_aid", &cost);
}

#[test]
fn profile_claim_aid() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, donor, recipient) = setup(&env);
    let expiry = env.ledger().sequence() + 10_000;
    let aid_id = client.create_aid(&donor, &recipient, &100, &expiry, &None);

    let (result, cost) = measure(&env, || client.try_claim_aid(&aid_id, &recipient));

    assert!(result.is_ok(), "the profiled claim_aid call must succeed");
    report("aid.claim_aid", &cost);
}
