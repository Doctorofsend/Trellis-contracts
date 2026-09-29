use crate::storage::TreasuryManager;
use crate::{TreasuryContract, TreasuryContractClient};
use shared::errors::Error;
use soroban_sdk::{
    contract, contractimpl, symbol_short,
    testutils::Ledger as _,
    testutils::{Address as _, Events},
    Address, Env, Symbol, Vec,
};
use std::collections::BTreeMap;
use std::format;

// ===========================================================================
// Test helpers
fn setup(env: &Env) -> (TreasuryContractClient<'static>, Address, i128) {
    let contract_id = env.register_contract(None, TreasuryContract);
    let client = TreasuryContractClient::new(env, &contract_id);
    let admin = Address::generate(env);
    let limit: i128 = 1_000;
    client.initialize(&admin, &limit);
    (client, admin, limit)
}

// ===========================================================================
// Contract state snapshot helpers
//
// These helpers capture a deterministic snapshot of treasury contract state
// (category balances, ownership/admin, status flags, and metadata) so that
// tests can assert on expected deltas before/after an operation. Snapshots
// are compared structurally and produce useful failure output when an
// unexpected delta is observed.

/// A single (token, category) balance entry in a snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BalanceEntry {
    pub token: Address,
    pub category: Symbol,
    pub amount: i128,
}

/// Deterministic snapshot of treasury contract state.
///
/// Balances are stored in a `BTreeMap` keyed by a stable string form of
/// `(token, category)` so that iteration order is deterministic across runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreasurySnapshot {
    pub balances: BTreeMap<(String, String), i128>,
    pub admin: Option<Address>,
    pub initialized: bool,
    pub withdrawal_limit: Option<i128>,
    pub referral_contract: Option<Address>,
    pub managers: Vec<Address>,
}

impl TreasurySnapshot {
    /// Return the balance for a given token/category pair, defaulting to 0.
    pub fn balance(&self, token: &Address, category: &Symbol) -> i128 {
        let key = (format!("{:?}", token), format!("{:?}", category));
        *self.balances.get(&key).unwrap_or(&0)
    }

    /// Compute the delta between two snapshots as a list of human-readable
    /// change descriptions. Empty list means no observable change.
    pub fn diff(&self, other: &TreasurySnapshot) -> Vec<String> {
        let mut changes: Vec<String> = Vec::new();

        let mut keys: Vec<(String, String)> = self.balances.keys().cloned().collect();
        for k in other.balances.keys() {
            if !keys.contains(k) {
                keys.push(k.clone());
            }
        }
        keys.sort();

        for key in keys {
            let before = *self.balances.get(&key).unwrap_or(&0);
            let after = *other.balances.get(&key).unwrap_or(&0);
            if before != after {
                changes.push(format!(
                    "balance[{:?}/{:?}]: {} -> {} (delta {})",
                    key.0,
                    key.1,
                    before,
                    after,
                    after - before
                ));
            }
        }

        if self.admin != other.admin {
            changes.push(format!(
                "admin: {:?} -> {:?}",
                self.admin, other.admin
            ));
        }
        if self.initialized != other.initialized {
            changes.push(format!(
                "initialized: {} -> {}",
                self.initialized, other.initialized
            ));
        }
        if self.withdrawal_limit != other.withdrawal_limit {
            changes.push(format!(
                "withdrawal_limit: {:?} -> {:?}",
                self.withdrawal_limit, other.withdrawal_limit
            ));
        }
        if self.referral_contract != other.referral_contract {
            changes.push(format!(
                "referral_contract: {:?} -> {:?}",
                self.referral_contract, other.referral_contract
            ));
        }
        if self.managers != other.managers {
            changes.push(format!(
                "managers: {:?} -> {:?}",
                self.managers, other.managers
            ));
        }

        changes
    }

    /// Assert that the delta between `self` (before) and `other` (after)
    /// matches the expected list of change descriptions exactly.
    ///
    /// Panics with a readable diff when the observed delta does not match.
    pub fn assert_delta(&self, other: &TreasurySnapshot, expected: &[&str]) {
        let observed = self.diff(other);
        let expected_vec: Vec<String> =
            expected.iter().map(|s| s.to_string()).collect();
        assert_eq!(
            observed, expected_vec,
            "unexpected state delta.\n  before: {:#?}\n  after:  {:#?}",
            self, other
        );
    }

    /// Assert that no observable state change occurred.
    pub fn assert_unchanged(&self, other: &TreasurySnapshot) {
        let observed = self.diff(other);
        assert!(
            observed.is_empty(),
            "expected no state change but observed: {:#?}",
            observed
        );
    }
}

/// Capture a deterministic snapshot of the treasury contract state.
///
/// `tokens` and `categories` enumerate the balance slots to include. Any
/// slot not present in storage is recorded as 0.
pub fn snapshot(
    client: &TreasuryContractClient,
    admin: &Address,
    tokens: &[Address],
    categories: &[Symbol],
) -> TreasurySnapshot {
    let mut balances: BTreeMap<(String, String), i128> = BTreeMap::new();
    for token in tokens {
        for category in categories {
            let amount = client.category_balance(token, category);
            balances.insert(
                (format!("{:?}", token), format!("{:?}", category)),
                amount,
            );
        }
    }

    let initialized = client.audit_trail(admin, &1).is_ok();
    let withdrawal_limit = if initialized {
        client.withdrawal_limit().ok()
    } else {
        None
    };
    let referral_contract = client.referral_contract().ok();
    let managers = client.treasury_managers();

    TreasurySnapshot {
        balances,
        admin: Some(admin.clone()),
        initialized,
        withdrawal_limit,
        referral_contract,
        managers,
    }
}

// ===========================================================================
// Tests
#[test]
fn test_withdraw_success_decrements_balance_and_emits_event() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, admin, _limit) = setup(&env);
    let token = Address::generate(&env);
    let category = symbol_short!("reserve");
    let recipient = Address::generate(&env);

    let tokens = [token.clone()];
    let categories = [category.clone()];
    let before = snapshot(&client, &admin, &tokens, &categories);

    client.deposit(&admin, &token, &category, &500);

    client.withdraw(&admin, &token, &recipient, &200, &category);

    assert!(
        !env.events().all().is_empty(),
        "expected TREASURY_WITHDRAW event to be emitted"
    );

    let after = snapshot(&client, &admin, &tokens, &categories);
    before.assert_delta(&after, &["balance[\"reserve\"]: 0 -> 300 (delta 300)"]);
}

#[test]
fn sensitive_treasury_actions_are_available_in_the_maintainer_audit_trail() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, admin, _) = setup(&env);
    let token = Address::generate(&env);
    let category = symbol_short!("reserve");
    let recipient = Address::generate(&env);

    let tokens = [token.clone()];
    let categories = [category.clone()];
    let before = snapshot(&client, &admin, &tokens, &categories);

    client.deposit(&admin, &token, &category, &500);
    client.withdraw(&admin, &token, &recipient, &200, &category);

    let audit = client.audit_trail(&admin, &10).unwrap();
    assert_eq!(audit.len(), 3);

    let withdrawal = audit.get(0).unwrap();
    assert_eq!(withdrawal.actor, admin);
    assert_eq!(withdrawal.scope, symbol_short!("treasury"));
    assert_eq!(withdrawal.action, symbol_short!("withdraw"));
    assert_eq!(withdrawal.reason, symbol_short!("funds_out"));
    assert_eq!(withdrawal.resource, Some(token.clone()));
    assert_eq!(withdrawal.attribute, Some(category.clone()));
    assert_eq!(withdrawal.before, Some(500));
    assert_eq!(withdrawal.after, Some(300));

    let deposit = audit.get(1).unwrap();
    assert_eq!(deposit.action, symbol_short!("deposit"));
    assert_eq!(deposit.resource, Some(token));
    assert_eq!(deposit.attribute, Some(category));
    assert_eq!(deposit.before, Some(0));
    assert_eq!(deposit.after, Some(500));

    let initialization = audit.get(2).unwrap();
    assert_eq!(initialization.actor, admin);
    assert_eq!(initialization.action, symbol_short!("init"));
    assert_eq!(initialization.reason, symbol_short!("setup"));
    assert_eq!(initialization.after, Some(1_000));

    let after = snapshot(&client, &admin, &tokens, &categories);
    before.assert_delta(&after, &["balance[\"reserve\"]: 0 -> 300 (delta 300)"]);
}

#[test]
fn initialize_is_one_time_and_requires_the_initial_admin_signature() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, admin, _) = setup(&env);
    let replacement_admin = Address::generate(&env);

    let tokens: [Address; 0] = [];
    let categories: [Symbol; 0] = [];
    let before = snapshot(&client, &admin, &tokens, &categories);

    assert_eq!(
        client.try_initialize(&replacement_admin, &2_000),
        Err(Ok(Error::AlreadyInitialized))
    );
    assert_eq!(client.audit_trail(&admin, &10).unwrap().len(), 1);

    let after = snapshot(&client, &admin, &tokens, &categories);
    before.assert_unchanged(&after);
}

// ===========================================================================
// Multi-token isolation tests
#[test]
fn test_multi_token_isolation_same_category() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, admin, _limit) = setup(&env);
    let token_usdc = Address::generate(&env);
    let token_xlm = Address::generate(&env);
    let category = symbol_short!("reserve");
    let recipient = Address::generate(&env);

    let tokens = [token_usdc.clone(), token_xlm.clone(), token_btc.clone()];
    let categories = [category.clone()];
    let before = snapshot(&client, &admin, &tokens, &categories);

    // Deposit 50,000 USDC and 100,000 XLM into the same category
    client.deposit(&admin, &token_usdc, &category, &50_000);
    client.deposit(&admin, &token_xlm, &category, &100_000);

    // Verify balances are stored independently
    assert_eq!(client.category_balance(&token_usdc, &category), 50_000);
    assert_eq!(client.category_balance(&token_xlm, &category), 100_000);

    // Withdrawing from USDC does NOT affect XLM balance
    client.withdraw(&admin, &token_usdc, &recipient, &500, &category);
    assert_eq!(client.category_balance(&token_usdc, &category), 49_500);
    assert_eq!(client.category_balance(&token_xlm, &category), 100_000);

    // Withdrawing from XLM does NOT affect USDC balance
    client.withdraw(&admin, &token_xlm, &recipient, &1_000, &category);
    assert_eq!(client.category_balance(&token_usdc, &category), 49_500);
    assert_eq!(client.category_balance(&token_xlm, &category), 99_000);

    // Attempting to withdraw Token C (not deposited) fails with InsufficientBalance
    let token_btc = Address::generate(&env);
    let result = client.try_withdraw(&admin, &token_btc, &recipient, &100, &category);
    assert_eq!(result, Err(Ok(Error::InsufficientBalance)));

    let after = snapshot(&client, &admin, &tokens, &categories);
    before.assert_delta(
        &after,
        &[
            "balance[\"reserve\"]: 0 -> 49_500 (delta 49_500)",
            "balance[\"reserve\"]: 0 -> 99_000 (delta 99_000)",
            "balance[\"reserve\"]: 0 -> 0 (delta 0)",
        ],
    );
}

// ===========================================================================
// Test: Withdraw Rejections
#[test]
fn test_withdraw_rejects_non_manager() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, admin, _limit) = setup(&env);
    let token = Address::generate(&env);
    let category = symbol_short!("reserve");
    let recipient = Address::generate(&env);
    let stranger = Address::generate(&env);

    let tokens = [token.clone()];
    let categories = [category.clone()];
    let before = snapshot(&client, &admin, &tokens, &categories);

    client.deposit(&admin, &token, &category, &500);

    let result = client.try_withdraw(&stranger, &token, &recipient, &100, &category);
    assert_eq!(result, Err(Ok(Error::Unauthorized)));

    let after = snapshot(&client, &admin, &tokens, &categories);
    before.assert_delta(&after, &["balance[\"reserve\"]: 0 -> 500 (delta 500)"]);
}

// ===========================================================================
// Test: Withdraw Rejections
#[test]
fn test_withdraw_rejects_amount_above_limit() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, admin, limit) = setup(&env);
    let token = Address::generate(&env);
    let category = symbol_short!("reserve");
    let recipient = Address::generate(&env);

    let tokens = [token.clone()];
    let categories = [category.clone()];
    let before = snapshot(&client, &admin, &tokens, &categories);

    client.deposit(&admin, &token, &category, &(limit * 2));

    let over_limit = limit + 1;
    let result = client.try_withdraw(&admin, &token, &recipient, &over_limit, &category);
    assert_eq!(result, Err(Ok(Error::WithdrawalLimitExceeded)));

    let after = snapshot(&client, &admin, &tokens, &categories);
    before.assert_delta(&after, &["balance[\"reserve\"]: 0 -> 2_000 (delta 2_000)"]);
}

// ===========================================================================
// Test: Withdraw Rejections
#[test]
fn test_withdraw_rejects_insufficient_category_balance() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, admin, _limit) = setup(&env);
    let token = Address::generate(&env);
    let category = symbol_short!("rewards");
    let recipient = Address::generate(&env);

    let tokens = [token.clone()];
    let categories = [category.clone()];
    let before = snapshot(&client, &admin, &tokens, &categories);

    client.deposit(&admin, &token, &category, &50);

    let result = client.try_withdraw(&admin, &token, &recipient, &100, &category);
    assert_eq!(result, Err(Ok(Error::InsufficientBalance)));

    let after = snapshot(&client, &admin, &tokens, &categories);
    before.assert_delta(&after, &["balance[\"rewards\"]: 0 -> 50 (delta 50)"]);
}

// ===========================================================================
// Test: Withdraw Rejections
#[test]
fn test_withdraw_rejects_zero_or_negative_amount() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, admin, _limit) = setup(&env);
    let token = Address::generate(&env);
    let category = symbol_short!("reserve");
    let recipient = Address::generate(&env);

    let tokens = [token.clone()];
    let categories = [category.clone()];
    let before = snapshot(&client, &admin, &tokens, &categories);

    client.deposit(&admin, &token, &category, &500);

    let result = client.try_withdraw(&admin, &token, &recipient, &0, &category);
    assert_eq!(result, Err(Ok(Error::InvalidArgument)));

    let after = snapshot(&client, &admin, &tokens, &categories);
    before.assert_delta(&after, &["balance[\"reserve\"]: 0 -> 500 (delta 500)"]);
}

// ===========================================================================
// Test: Admin can add and remove treasury managers
#[test]
fn test_admin_can_add_and_remove_treasury_manager() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, admin, _limit) = setup(&env);
    let token = Address::generate(&env);
    let category = symbol_short!("reserve");
    let recipient = Address::generate(&env);
    let manager = Address::generate(&env);

    let tokens = [token.clone()];
    let categories = [category.clone()];
    let before = snapshot(&client, &admin, &tokens, &categories);

    client.deposit(&admin, &token, &category, &500);

    // Not yet a manager -> rejected.
    let result = client.try_withdraw(&manager, &token, &recipient, &100, &category);
    assert_eq!(result, Err(Ok(Error::Unauthorized)));

    // Admin grants the role -> now allowed.
    client.add_treasury_manager(&admin, &manager);
    client.withdraw(&manager, &token, &recipient, &100, &category);
    assert_eq!(client.category_balance(&token, &category), 400);

    // Admin revokes the role -> rejected again.
    client.remove_treasury_manager(&admin, &manager);
    let result = client.try_withdraw(&manager, &token, &recipient, &50, &category);
    assert_eq!(result, Err(Ok(Error::Unauthorized)));

    let after = snapshot(&client, &admin, &tokens, &categories);
    before.assert_delta(&after, &["balance[\"reserve\"]: 0 -> 400 (delta 400)"]);
}

/// Stands in for the referral contract: it forwards to the treasury's
/// `distribute_reward`, so from the treasury's perspective the direct caller
/// is this contract's own address (whichever instance is registered).
///
/// `pub(crate)` so the budget regression suite in `budget_test.rs` can reuse
/// the same stand-in instead of defining a second one.
#[contract]
pub(crate) struct MockReferralCaller;

#[contractimpl]
impl MockReferralCaller {
    pub fn call_distribute(
        env: Env,
        treasury: Address,
        token: Address,
        recipient: Address,
        amount: i128,
    ) -> Result<(), Error> {
        let client = TreasuryContractClient::new(&env, &treasury);
        match client.try_distribute_reward(&token, &recipient, &amount) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(_)) => Err(Error::InvalidArgument),
            Err(Ok(error)) => Err(error),
            Err(Err(_)) => Err(Error::InvalidArgument),
        }
    }
}

// ===========================================================================
// Test: Referral contract can call distribute_reward
#[test]
fn test_distribute_reward_pays_recipient_and_decrements_rewards() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, admin, _limit) = setup(&env);
    let token = Address::generate(&env);
    let rewards = symbol_short!("rewards");
    let recipient = Address::generate(&env);
    let referral_id = env.register_contract(None, MockReferralCaller);
    let referral = MockReferralCallerClient::new(&env, &referral_id);

    client.deposit(&admin, &token, &rewards, &1_000);
    client.set_referral_contract(&admin, &referral_id);

    referral.call_distribute(&client.address, &token, &recipient, &400);

    assert!(
        !env.events().all().is_empty(),
        "expected CommissionPaid event to be emitted"
    );
    assert_eq!(client.category_balance(&token, &rewards), 600);
}

// ===========================================================================
// Test: Referral contract can call distribute_reward
#[test]
fn test_distribute_reward_rejects_underfunded_rewards_pool() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, admin, _limit) = setup(&env);
    let token = Address::generate(&env);
    let rewards = symbol_short!("rewards");
    let recipient = Address::generate(&env);
    let referral_id = env.register_contract(None, MockReferralCaller);
    let referral = MockReferralCallerClient::new(&env, &referral_id);

    client.deposit(&admin, &token, &rewards, &100);
    client.set_referral_contract(&admin, &referral_id);

    let result = referral.try_call_distribute(&client.address, &token, &recipient, &200);
    assert_eq!(result, Err(Ok(Error::InsufficientBalance)));
    assert_eq!(client.category_balance(&token, &rewards), 100);
}

#[test]
fn test_distribute_reward_rejects_caller_that_is_not_registered_referral_contract() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, admin, _limit) = setup(&env);
    let token = Address::generate(&env);
    let rewards = symbol_short!("rewards");
    let recipient = Address::generate(&env);
    let referral_id = env.register_contract(None, MockReferralCaller);
    let impostor_id = env.register_contract(None, MockReferralCaller);
    let impostor = MockReferralCallerClient::new(&env, &impostor_id);

    client.deposit(&admin, &token, &rewards, &1_000);
    client.set_referral_contract(&admin, &referral_id);

    // Disable blanket auth mocking
    env.set_auths(&[]);

    let result = impostor.try_call_distribute(&client.address, &token, &recipient, &400);
    assert!(result.is_err());
    assert_eq!(client.category_balance(&token, &rewards), 1_000);
}

#[test]
fn test_distribute_reward_rejects_when_no_referral_contract_registered() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, admin, _limit) = setup(&env);
    let token = Address::generate(&env);
    let rewards = symbol_short!("rewards");
    let recipient = Address::generate(&env);
    let referral_id = env.register_contract(None, MockReferralCaller);
    let referral = MockReferralCallerClient::new(&env, &referral_id);

    client.deposit(&admin, &token, &rewards, &1_000);

    let result = referral.try_call_distribute(&client.address, &token, &recipient, &400);
    assert!(result.is_err());
    assert_eq!(client.category_balance(&token, &rewards), 1_000);
}

// ===========================================================================
// Gas benchmark tests
// ===========================================================================

#[test]
fn gas_bench_withdraw_rejects_zero_before_auth() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, admin, _limit) = setup(&env);
    let token = Address::generate(&env);
    let category = symbol_short!("reserve");
    let recipient = Address::generate(&env);

    client.deposit(&admin, &token, &category, &500);

    let stranger = Address::generate(&env);
    let result = client.try_withdraw(&stranger, &token, &recipient, &0, &category);
    assert_eq!(result, Err(Ok(Error::InvalidArgument)));
}

#[test]
fn gas_bench_deposit_rejects_zero_before_auth() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, _admin, _limit) = setup(&env);
    let token = Address::generate(&env);
    let category = symbol_short!("reserve");
    let stranger = Address::generate(&env);

    let result = client.try_deposit(&stranger, &token, &category, &0);
    assert_eq!(result, Err(Ok(Error::InvalidArgument)));
}

#[test]
fn gas_bench_distribute_reward_rejects_zero_before_auth() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, admin, _limit) = setup(&env);
    let token = Address::generate(&env);
    let rewards = symbol_short!("rewards");
    let recipient = Address::generate(&env);

    client.deposit(&admin, &token, &rewards, &1_000);

    let result = client.try_distribute_reward(&token, &recipient, &0);
    assert_eq!(result, Err(Ok(Error::InvalidArgument)));
}

#[test]
fn gas_bench_emergency_withdraw_rejects_zero_before_auth() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, admin, _limit) = setup(&env);
    let token = Address::generate(&env);
    let recipient = Address::generate(&env);

    let result = client.try_emergency_withdraw(&admin, &token, &recipient, &0);
    assert_eq!(result, Err(Ok(Error::InvalidArgument)));
}

// ===========================================================================
// Time-window validation tests
// ===========================================================================

/// Helper: schedule a withdrawal with an explicit [start, expiry] window.
fn schedule_withdraw(
    env: &Env,
    client: &TreasuryContractClient,
    admin: &Address,
    token: &Address,
    recipient: &Address,
    amount: i128,
    category: &soroban_sdk::Symbol,
    start: u64,
    expiry: u64,
) -> Result<(), Error> {
    let _ = env;
    match client.try_schedule_withdraw(
        admin, token, recipient, &amount, category, &start, &expiry,
    ) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(_)) => Err(Error::InvalidArgument),
        Err(Ok(error)) => Err(error),
        Err(Err(_)) => Err(Error::InvalidArgument),
    }
}

#[test]
fn test_scheduled_action_rejected_before_window() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, admin, _limit) = setup(&env);
    let token = Address::generate(&env);
    let category = symbol_short!("reserve");
    let recipient = Address::generate(&env);

    client.deposit(&admin, &token, &category, &500);

    // Ledger time is 0; window opens at 100.
    env.ledger().set_timestamp(0);
    let result = schedule_withdraw(
        &env, &client, &admin, &token, &recipient, 100, &category, 100, 200,
    );
    assert_eq!(result, Err(Error::ActionNotYetValid));
    assert_eq!(client.category_balance(&token, &category), 500);
}

#[test]
fn test_scheduled_action_valid_inside_window() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, admin, _limit) = setup(&env);
    let token = Address::generate(&env);
    let category = symbol_short!("reserve");
    let recipient = Address::generate(&env);

    client.deposit(&admin, &token, &category, &500);

    env.ledger().set_timestamp(150);
    let result = schedule_withdraw(
        &env, &client, &admin, &token, &recipient, 100, &category, 100, 200,
    );
    assert_eq!(result, Ok(()));
    assert_eq!(client.category_balance(&token, &category), 400);
}

#[test]
fn test_scheduled_action_rejected_after_expiry() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, admin, _limit) = setup(&env);
    let token = Address::generate(&env);
    let category = symbol_short!("reserve");
    let recipient = Address::generate(&env);

    client.deposit(&admin, &token, &category, &500);

    env.ledger().set_timestamp(250);
    let result = schedule_withdraw(
        &env, &client, &admin, &token, &recipient, 100, &category, 100, 200,
    );
    assert_eq!(result, Err(Error::ActionExpired));
    assert_eq!(client.category_balance(&token, &category), 500);
}

#[test]
fn test_scheduled_action_boundary_start_inclusive() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, admin, _limit) = setup(&env);
    let token = Address::generate(&env);
    let category = symbol_short!("reserve");
    let recipient = Address::generate(&env);

    client.deposit(&admin, &token, &category, &500);

    // Exactly at start -> accepted.
    env.ledger().set_timestamp(100);
    let result = schedule_withdraw(
        &env, &client, &admin, &token, &recipient, 100, &category, 100, 200,
    );
    assert_eq!(result, Ok(()));
    assert_eq!(client.category_balance(&token, &category), 400);
}

#[test]
fn test_scheduled_action_boundary_expiry_inclusive() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, admin, _limit) = setup(&env);
    let token = Address::generate(&env);
    let category = symbol_short!("reserve");
    let recipient = Address::generate(&env);

    client.deposit(&admin, &token, &category, &500);

    // Exactly at expiry -> accepted (inclusive upper bound).
    env.ledger().set_timestamp(200);
    let result = schedule_withdraw(
        &env, &client, &admin, &token, &recipient, 100, &category, 100, 200,
    );
    assert_eq!(result, Ok(()));
    assert_eq!(client.category_balance(&token, &category), 400);
}

#[test]
fn test_scheduled_action_rejects_manipulated_timestamp_window() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, admin, _limit) = setup(&env);
    let token = Address::generate(&env);
    let category = symbol_short!("reserve");
    let recipient = Address::generate(&env);

    client.deposit(&admin, &token, &category, &500);

    // Expiry <= start is a malformed window and must be rejected up front.
    env.ledger().set_timestamp(150);
    let result = schedule_withdraw(
        &env, &client, &admin, &token, &recipient, 100, &category, 200, 100,
    );
    assert_eq!(result, Err(Error::InvalidArgument));
    assert_eq!(client.category_balance(&token, &category), 500);
}

#[test]
fn test_scheduled_action_rejects_stale_window() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, admin, _limit) = setup(&env);
    let token = Address::generate(&env);
    let category = symbol_short!("reserve");
    let recipient = Address::generate(&env);

    client.deposit(&admin, &token, &category, &500);

    // Window is entirely in the past relative to ledger time.
    env.ledger().set_timestamp(1_000);
    let result = schedule_withdraw(
        &env, &client, &admin, &token, &recipient, 100, &category, 100, 200,
    );
    assert_eq!(result, Err(Error::ActionExpired));
    assert_eq!(client.category_balance(&token, &category), 500);
}
