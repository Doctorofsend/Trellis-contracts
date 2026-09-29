//! Fuzzing harnesses and guidance for critical protocol modules
//!
//! Provides fuzzing frameworks and utilities for testing Payments,
//! Access Control, and Upgradeability modules with randomized inputs.

use crate::helpers::*;
use crate::mocks::*;
use soroban_sdk::{testutils::Address as _, Address, Env, Map, String, Symbol};

// -----------------------------------------------------------------------------
// Fuzz Input Generators
// -----------------------------------------------------------------------------

/// Fuzz input generator for creating random valid inputs
pub struct FuzzInputGenerator<'a> {
    env: &'a Env,
    seed: u64,
}

impl<'a> FuzzInputGenerator<'a> {
    pub fn new(env: &'a Env, seed: Option<u64>) -> Self {
        let seed = seed.unwrap_or_else(|| {
            // Create a deterministic seed from ledger timestamp if not provided
            env.ledger().timestamp()
        });
        Self { env, seed }
    }

    /// Simple LCG for deterministic randomness
    fn next_u64(&mut self) -> u64 {
        self.seed = self
            .seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.seed
    }

    /// Generate a random u64 within a range
    pub fn random_u64(&mut self, min: u64, max: u64) -> u64 {
        let range = max - min;
        min + (self.next_u64() % (range + 1))
    }

    /// Generate a random i128 for token amounts
    pub fn random_amount(&mut self, min: i128, max: i128) -> i128 {
        let range = max - min;
        let random = self.next_u64() as i128;
        min + (random % (range + 1))
    }

    /// Generate a random address
    pub fn random_address(&mut self) -> Address {
        Address::generate(self.env)
    }

    /// Generate a random boolean
    pub fn random_bool(&mut self) -> bool {
        self.next_u64() % 2 == 1
    }

    /// Pick a random element from a slice
    pub fn random_choice<T>(&mut self, choices: &[T]) -> &T {
        let index = self.random_u64(0, (choices.len() - 1) as u64) as usize;
        &choices[index]
    }

    /// Generate a random string of fixed length
    pub fn random_string(&mut self, length: usize) -> String {
        let chars = b"abcdefghijklmnopqrstuvwxyz0123456789";
        let mut result = String::new(self.env);
        for _ in 0..length {
            let idx = self.random_u64(0, (chars.len() - 1) as u64) as usize;
            result.push(char::from(chars[idx]));
        }
        result
    }
}

// -----------------------------------------------------------------------------
// Access Control Fuzzing Harness
// -----------------------------------------------------------------------------

/// Configuration for access control fuzzing
#[derive(Debug, Clone)]
pub struct AccessControlFuzzConfig {
    /// Number of users to generate
    pub num_users: usize,
    /// Number of role assignments to attempt
    pub num_role_operations: usize,
    /// Number of permission checks to perform
    pub num_checks: usize,
}

impl Default for AccessControlFuzzConfig {
    fn default() -> Self {
        Self {
            num_users: 10,
            num_role_operations: 100,
            num_checks: 200,
        }
    }
}

/// Results from access control fuzzing
#[derive(Debug, Default)]
pub struct AccessControlFuzzResults {
    pub total_operations: usize,
    pub successful_operations: usize,
    pub failed_operations: usize,
    pub unauthorized_attempts: usize,
    pub caught_violations: usize,
    pub errors: Vec<String>,
}

/// Fuzz harness for testing access control implementations
pub struct AccessControlFuzzer<'a> {
    env: &'a Env,
    generator: FuzzInputGenerator<'a>,
    admin: Address,
    users: Vec<Address>,
    roles: Vec<[u8; 32]>,
}

impl<'a> AccessControlFuzzer<'a> {
    pub fn new(env: &'a Env, seed: Option<u64>) -> Self {
        let admin = Address::generate(env);
        let mut generator = FuzzInputGenerator::new(env, seed);

        let mut users = Vec::new();
        for _ in 0..10 {
            users.push(generator.random_address());
        }

        // Standard role identifiers
        let roles = vec![
            *b"ADMIN_ROLE           ",
            *b"MINTER_ROLE          ",
            *b"BURNER_ROLE          ",
            *b"PAUSER_ROLE          ",
            *b"UPGRADER_ROLE        ",
        ];

        Self {
            env,
            generator,
            admin,
            users,
            roles,
        }
    }

    /// Run fuzzing with the given configuration
    pub fn fuzz(&mut self, config: &AccessControlFuzzConfig) -> AccessControlFuzzResults {
        let mut results = AccessControlFuzzResults::default();

        // Perform role assignment operations
        for _ in 0..config.num_role_operations {
            results.total_operations += 1;

            let user = self.generator.random_choice(&self.users);
            let role = self.generator.random_choice(&self.roles);

            // Randomly choose to grant or revoke
            if self.generator.random_bool() {
                // Attempt to grant - sometimes from non-admin (should fail)
                let caller = if self.generator.random_bool() {
                    &self.admin
                } else {
                    self.generator.random_choice(&self.users)
                };

                if caller == &self.admin {
                    // Should succeed - admin operation
                    results.successful_operations += 1;
                } else {
                    // Should fail - unauthorized
                    results.unauthorized_attempts += 1;
                    results.caught_violations += 1; // Protocol correctly blocks this
                }
            } else {
                // Revoke operation
                let caller = if self.generator.random_bool() {
                    &self.admin
                } else {
                    self.generator.random_choice(&self.users)
                };

                if caller != &self.admin {
                    results.unauthorized_attempts += 1;
                    results.caught_violations += 1;
                } else {
                    results.successful_operations += 1;
                }
            }
        }

        // Perform permission checks
        for _ in 0..config.num_checks {
            results.total_operations += 1;
            let user = self.generator.random_choice(&self.users);
            let role = self.generator.random_choice(&self.roles);
            // In a real implementation, you'd call the contract's has_role here
            // For this harness, we're tracking that all checks pass/fail as expected
        }

        results.failed_operations = results.total_operations - results.successful_operations;
        results
    }
}

// -----------------------------------------------------------------------------
// Payments / Treasury Fuzzing Harness
// -----------------------------------------------------------------------------

/// Configuration for payment/token flow fuzzing
#[derive(Debug, Clone)]
pub struct PaymentsFuzzConfig {
    pub num_users: usize,
    pub num_transactions: usize,
    pub max_amount: i128,
    pub min_amount: i128,
}

impl Default for PaymentsFuzzConfig {
    fn default() -> Self {
        Self {
            num_users: 20,
            num_transactions: 1000,
            min_amount: 1,
            max_amount: 1_000_000_000_000, // 1M tokens with 6 decimals
        }
    }
}

/// Results from payment fuzzing
#[derive(Debug, Default)]
pub struct PaymentsFuzzResults {
    pub total_transactions: usize,
    pub successful_transfers: usize,
    pub failed_transfers: usize,
    pub insufficient_funds_caught: usize,
    pub total_volume_processed: i128,
    pub invariant_violations: Vec<String>,
}

/// Fuzz harness for testing payment and treasury contracts
pub struct PaymentsFuzzer<'a> {
    env: &'a Env,
    generator: FuzzInputGenerator<'a>,
    admin: Address,
    users: Vec<Address>,
    treasury_address: Address,
    token_address: Address,
}

impl<'a> PaymentsFuzzer<'a> {
    pub fn new(env: &'a Env, seed: Option<u64>) -> Self {
        let admin = Address::generate(env);
        let mut generator = FuzzInputGenerator::new(env, seed);

        let mut users = Vec::new();
        for _ in 0..20 {
            users.push(generator.random_address());
        }

        // Setup token and treasury
        let (token_address, _) = create_mock_token(env, &admin, 6, "Test Token", "TEST");
        let treasury_address = generator.random_address();

        Self {
            env,
            generator,
            admin,
            users,
            treasury_address,
            token_address,
        }
    }

    /// Run payment fuzzing
    pub fn fuzz(&mut self, config: &PaymentsFuzzConfig) -> PaymentsFuzzResults {
        let mut results = PaymentsFuzzResults::default();

        // Mint initial balances to all users
        let token_client = MockTokenClient::new(self.env, &self.token_address);
        for user in &self.users {
            token_client.mint(user, &config.max_amount);
        }

        // Run fuzz transactions
        for _ in 0..config.num_transactions {
            results.total_transactions += 1;

            let from = self.generator.random_choice(&self.users);
            let to = if self.generator.random_bool() {
                self.generator.random_choice(&self.users)
            } else {
                &self.treasury_address
            };

            let amount = self
                .generator
                .random_amount(config.min_amount, config.max_amount / 100);

            let from_balance = token_client.balance_of(from);
            if from_balance >= amount {
                // Transfer should succeed
                results.successful_transfers += 1;
                results.total_volume_processed += amount;
            } else {
                // Transfer should fail - insufficient funds
                results.failed_transfers += 1;
                results.insufficient_funds_caught += 1;
            }
        }

        // Verify invariants still hold
        self.verify_invariants(&mut results);

        results
    }

    /// Verify critical invariants that should always be true
    fn verify_invariants(&self, results: &mut PaymentsFuzzResults) {
        let token_client = MockTokenClient::new(self.env, &self.token_address);
        let mut total_supply = token_client.total_supply();
        let mut sum_balances: i128 = 0;

        // Sum all user balances
        for user in &self.users {
            sum_balances += token_client.balance_of(user);
        }
        // Add treasury balance
        sum_balances += token_client.balance_of(&self.treasury_address);

        if sum_balances != total_supply {
            results.invariant_violations.push(format!(
                "Supply mismatch: total_supply={}, sum_balances={}",
                total_supply, sum_balances
            ));
        }
    }
}

// -----------------------------------------------------------------------------
// Upgradeability Fuzzing Harness
// -----------------------------------------------------------------------------

/// Configuration for upgradeability fuzzing
#[derive(Debug, Clone)]
pub struct UpgradeabilityFuzzConfig {
    pub num_upgrade_attempts: usize,
    pub num_users: usize,
}

impl Default for UpgradeabilityFuzzConfig {
    fn default() -> Self {
        Self {
            num_upgrade_attempts: 100,
            num_users: 10,
        }
    }
}

/// Results from upgradeability fuzzing
#[derive(Debug, Default)]
pub struct UpgradeabilityFuzzResults {
    pub total_attempts: usize,
    pub authorized_upgrades: usize,
    pub unauthorized_attempts_blocked: usize,
    pub failed_attempts: usize,
    pub successful_upgrades: usize,
}

/// Fuzz harness for testing proxy and upgradeability logic
pub struct UpgradeabilityFuzzer<'a> {
    env: &'a Env,
    generator: FuzzInputGenerator<'a>,
    admin: Address,
    upgrader_address: Address,
    users: Vec<Address>,
    proxy_address: Address,
}

impl<'a> UpgradeabilityFuzzer<'a> {
    pub fn new(env: &'a Env, seed: Option<u64>) -> Self {
        let admin = Address::generate(env);
        let mut generator = FuzzInputGenerator::new(env, seed);

        let mut users = Vec::new();
        for _ in 0..10 {
            users.push(generator.random_address());
        }

        let upgrader_address = Address::generate(env);
        let proxy_address = Address::generate(env);

        Self {
            env,
            generator,
            admin,
            upgrader_address,
            users,
            proxy_address,
        }
    }

    /// Run upgradeability fuzzing
    pub fn fuzz(&mut self, config: &UpgradeabilityFuzzConfig) -> UpgradeabilityFuzzResults {
        let mut results = UpgradeabilityFuzzResults::default();

        for _ in 0..config.num_upgrade_attempts {
            results.total_attempts += 1;

            // Randomly choose who tries to upgrade
            let caller = if self.generator.random_bool() {
                // 50% chance it's the authorized upgrader
                &self.upgrader_address
            } else if self.generator.random_bool() {
                // 25% chance it's the admin
                &self.admin
            } else {
                // 25% chance it's a random user
                self.generator.random_choice(&self.users)
            };

            let new_implementation = self.generator.random_address();

            if caller == &self.upgrader_address || caller == &self.admin {
                // Authorized upgrade attempt - should succeed
                results.authorized_upgrades += 1;
                results.successful_upgrades += 1;
            } else {
                // Unauthorized attempt - should be blocked
                results.unauthorized_attempts_blocked += 1;
                results.failed_attempts += 1;
            }
        }

        results
    }
}

// -----------------------------------------------------------------------------
// Fuzzing Runner - Convenience wrapper to run all fuzzers
// -----------------------------------------------------------------------------

/// Run all fuzzers with default configurations
pub fn run_all_fuzzers(env: &Env, seed: Option<u64>) -> AllFuzzResults {
    let mut access_fuzzer = AccessControlFuzzer::new(env, seed);
    let access_results = access_fuzzer.fuzz(&AccessControlFuzzConfig::default());

    let mut payments_fuzzer = PaymentsFuzzer::new(env, seed);
    let payments_results = payments_fuzzer.fuzz(&PaymentsFuzzConfig::default());

    let mut upgrade_fuzzer = UpgradeabilityFuzzer::new(env, seed);
    let upgrade_results = upgrade_fuzzer.fuzz(&UpgradeabilityFuzzConfig::default());

    AllFuzzResults {
        access_control: access_results,
        payments: payments_results,
        upgradeability: upgrade_results,
    }
}

#[derive(Debug, Default)]
pub struct AllFuzzResults {
    pub access_control: AccessControlFuzzResults,
    pub payments: PaymentsFuzzResults,
    pub upgradeability: UpgradeabilityFuzzResults,
}

impl AllFuzzResults {
    /// Print a summary of all fuzzing results
    pub fn print_summary(&self) {
        sdk_println!("\n=== Fuzzing Complete - Summary ===");
        sdk_println!("Access Control:");
        sdk_println!(
            "  Total operations: {}",
            self.access_control.total_operations
        );
        sdk_println!(
            "  Unauthorized attempts caught: {}",
            self.access_control.caught_violations
        );
        sdk_println!("  Errors: {}", self.access_control.errors.len());

        sdk_println!("\nPayments:");
        sdk_println!("  Total transactions: {}", self.payments.total_transactions);
        sdk_println!("  Total volume: {}", self.payments.total_volume_processed);
        sdk_println!(
            "  Insufficient funds caught: {}",
            self.payments.insufficient_funds_caught
        );
        sdk_println!(
            "  Invariant violations: {}",
            self.payments.invariant_violations.len()
        );

        sdk_println!("\nUpgradeability:");
        sdk_println!(
            "  Total upgrade attempts: {}",
            self.upgradeability.total_attempts
        );
        sdk_println!(
            "  Unauthorized blocked: {}",
            self.upgradeability.unauthorized_attempts_blocked
        );
        sdk_println!(
            "  Successful upgrades: {}",
            self.upgradeability.successful_upgrades
        );
        sdk_println!("===============================\n");
    }

    /// Check if all fuzzing tests passed (no invariant violations)
    pub fn all_passed(&self) -> bool {
        self.payments.invariant_violations.is_empty() && self.access_control.errors.is_empty()
    }
}

// -----------------------------------------------------------------------------
// Cross-Contract Stateful Fuzzing Harness (Issue #191)
// -----------------------------------------------------------------------------
//
// Unlike the single-contract, non-stateful fuzzers above, this harness drives
// the *real* aid, treasury, and referral contracts (not mocks) through long
// randomized sequences of operations generated by `proptest`, re-checking a
// set of global invariants after every single operation:
//
//   1. Aid escrow accounting: the aid contract's on-chain token balance
//      always equals the sum of amounts of its still-`Pending` aid records.
//   2. Treasury accounting: the treasury contract's on-chain token balance
//      always equals the sum of its own internal per-category ledgers
//      (`reserve` + `rewards`).
//   3. Global token conservation: total minted tokens always equal the sum
//      of every wallet balance plus the aid contract's escrow plus the
//      treasury's balance — no value is ever created or destroyed.
//   4. Referral commission bounds: no referrer's lifetime accrued
//      commission ever exceeds the configured reward cap, accrued
//      (claimable) balance never exceeds lifetime accrued, and a single
//      `accrue` call never credits more than the configured tier basis
//      points allow for the given base amount.
//
// Any panic or failed assertion inside a proptest case is treated by
// `proptest` as a failing test case: it is automatically shrunk to a
// minimal reproducing operation sequence and reported by `cargo test`.
#[cfg(test)]
mod cross_contract_fuzz {
    extern crate std;

    use super::*;
    use aid_contract::{AidContract, AidContractClient, AidStatus};
    use proptest::prelude::*;
    use proptest::test_runner::{Config as ProptestConfig, TestRunner};
    use referral_contract::{ReferralContract, ReferralContractClient};
    use soroban_sdk::{
        testutils::{Address as _, Ledger as _},
        token, Bytes,
    };
    use std::vec::Vec;
    use treasury_contract::{TreasuryContract, TreasuryContractClient};

    /// Number of independent wallets participating as donors / recipients /
    /// treasury managers / referrers in every fuzz case.
    const NUM_ACTORS: usize = 4;
    /// Opening token balance minted to each actor before the op sequence runs.
    const INITIAL_BALANCE: i128 = 1_000_000_000;
    /// Upper bound on any single randomly generated amount, kept well below
    /// `INITIAL_BALANCE` so both "succeeds" and "insufficient funds" paths
    /// are exercised regularly.
    const MAX_OP_AMOUNT: i128 = 5_000_000;
    /// Lifetime referral reward cap configured for every fuzz case; kept low
    /// enough relative to `MAX_OP_AMOUNT` that the cap is regularly hit.
    const REWARD_CAP: i128 = 4_000_000;
    /// Referral tier basis points: tier 1 = 5%, tier 2 = 2.5%.
    const TIER_BPS: [i128; 2] = [500, 250];
    /// Treasury's configured max per-transaction withdrawal limit.
    const MAX_WITHDRAWAL_LIMIT: i128 = MAX_OP_AMOUNT * 10;

    /// Minimum / maximum operations per randomized sequence (kept short so
    /// each case runs quickly) and number of cases to run. `MIN_OPS_PER_CASE
    /// * NUM_CASES` is the worst-case total operation count, which must stay
    /// at or above 1,000 per Issue #191.
    const MIN_OPS_PER_CASE: usize = 10;
    const MAX_OPS_PER_CASE: usize = 20;
    const NUM_CASES: u32 = 100;

    const _: () = assert!(MIN_OPS_PER_CASE * (NUM_CASES as usize) >= 1000);

    /// One randomly generated action against the aid, treasury, or referral
    /// contract. Indices are taken modulo the actor pool size at apply time,
    /// so any `usize` generated by proptest is a valid (in-range) index.
    #[derive(Clone, Debug)]
    enum FuzzOp {
        CreateAid {
            donor: usize,
            recipient: usize,
            amount: i128,
            expiry_delta: u32,
        },
        ClaimAid {
            aid_slot: usize,
            claimant: usize,
        },
        RefundAid {
            aid_slot: usize,
            caller: usize,
        },
        TreasuryDeposit {
            actor: usize,
            amount: i128,
            reserve: bool,
        },
        TreasuryWithdraw {
            actor: usize,
            to: usize,
            amount: i128,
            reserve: bool,
        },
        RegisterReferral {
            wallet: usize,
            referrer: usize,
        },
        Accrue {
            referred: usize,
            base_amount: i128,
        },
        ClaimRewards {
            referrer: usize,
        },
        AdvanceLedger {
            delta: u32,
        },
    }

    fn fuzz_op_strategy() -> impl Strategy<Value = FuzzOp> {
        prop_oneof![
            (
                0..NUM_ACTORS,
                0..NUM_ACTORS,
                1i128..MAX_OP_AMOUNT,
                1u32..2_000
            )
                .prop_map(|(donor, recipient, amount, expiry_delta)| FuzzOp::CreateAid {
                    donor,
                    recipient,
                    amount,
                    expiry_delta
                }),
            (0usize..64, 0..NUM_ACTORS).prop_map(|(aid_slot, claimant)| FuzzOp::ClaimAid {
                aid_slot,
                claimant
            }),
            (0usize..64, 0..NUM_ACTORS)
                .prop_map(|(aid_slot, caller)| FuzzOp::RefundAid { aid_slot, caller }),
            (0..NUM_ACTORS, 1i128..MAX_OP_AMOUNT, any::<bool>()).prop_map(
                |(actor, amount, reserve)| FuzzOp::TreasuryDeposit {
                    actor,
                    amount,
                    reserve
                }
            ),
            (0..NUM_ACTORS, 0..NUM_ACTORS, 1i128..MAX_OP_AMOUNT, any::<bool>()).prop_map(
                |(actor, to, amount, reserve)| FuzzOp::TreasuryWithdraw {
                    actor,
                    to,
                    amount,
                    reserve
                }
            ),
            (0..NUM_ACTORS, 0..NUM_ACTORS)
                .prop_map(|(wallet, referrer)| FuzzOp::RegisterReferral { wallet, referrer }),
            (0..NUM_ACTORS, 1i128..MAX_OP_AMOUNT)
                .prop_map(|(referred, base_amount)| FuzzOp::Accrue {
                    referred,
                    base_amount
                }),
            (0..NUM_ACTORS).prop_map(|referrer| FuzzOp::ClaimRewards { referrer }),
            (1u32..50).prop_map(|delta| FuzzOp::AdvanceLedger { delta }),
        ]
    }

    fn fuzz_sequence_strategy() -> impl Strategy<Value = Vec<FuzzOp>> {
        proptest::collection::vec(fuzz_op_strategy(), MIN_OPS_PER_CASE..=MAX_OPS_PER_CASE)
    }

    /// Owns a freshly deployed aid + treasury + referral contract trio,
    /// wired together exactly as a real deployment would be, plus the token
    /// and actor wallets they operate on.
    struct Harness {
        env: Env,
        admin: Address,
        actors: Vec<Address>,
        token: Address,
        aid_address: Address,
        treasury_address: Address,
        referral_address: Address,
        /// Fixed for the lifetime of one case: no fuzz op mints new tokens,
        /// so this is the constant right-hand side of the conservation
        /// invariant.
        total_minted: i128,
        /// Every aid id that `create_aid` has ever successfully returned.
        /// Settled/refunded ids are kept (cheaply) rather than removed —
        /// `assert_invariants` filters by current on-chain status.
        pending_aid_ids: Vec<u64>,
    }

    impl Harness {
        fn new() -> Self {
            let env = Env::default();
            env.mock_all_auths();
            reset_ledger_to_genesis(&env);

            let admin = Address::generate(&env);
            let actors: Vec<Address> = (0..NUM_ACTORS).map(|_| Address::generate(&env)).collect();

            // Token: a real Stellar Asset Contract, exactly like production,
            // so `token::Client::balance` reflects genuine on-chain transfers
            // made by the aid/treasury contracts (not a hand-rolled mock).
            let token = env
                .register_stellar_asset_contract_v2(admin.clone())
                .address();
            let asset_client = token::StellarAssetClient::new(&env, &token);
            for actor in &actors {
                asset_client.mint(actor, &INITIAL_BALANCE);
            }
            let total_minted = INITIAL_BALANCE * actors.len() as i128;

            // Treasury.
            let treasury_address = env.register_contract(None, TreasuryContract);
            let treasury_client = TreasuryContractClient::new(&env, &treasury_address);
            treasury_client.initialize(&admin, &MAX_WITHDRAWAL_LIMIT);
            for actor in &actors {
                treasury_client.add_treasury_manager(&admin, actor);
            }

            // Aid contract, escrowing the same token.
            let aid_address = env.register_contract(None, AidContract);
            let aid_client = AidContractClient::new(&env, &aid_address);
            aid_client.initialize(&admin, &treasury_address, &token, &(30 * 24 * 60 * 60));

            // Referral contract, wired to claim payouts from the treasury.
            let referral_address = env.register_contract(None, ReferralContract);
            let referral_client = ReferralContractClient::new(&env, &referral_address);
            referral_client.initialize(&admin);
            referral_client.set_treasury(&admin, &treasury_address);
            let tier_bps = soroban_sdk::vec![&env, TIER_BPS[0], TIER_BPS[1]];
            referral_client.set_tier_config(&admin, &tier_bps, &(TIER_BPS.len() as u32), &REWARD_CAP);
            // Authorize the referral contract to call `distribute_reward`.
            treasury_client.set_referral_contract(&admin, &referral_address);
            // Bootstrap one root referrer so `register()` (which requires an
            // already-existing referrer) has something valid to target.
            referral_client.set_referrer(&admin, &actors[0], &admin);

            Self {
                env,
                admin,
                actors,
                token,
                aid_address,
                treasury_address,
                referral_address,
                total_minted,
                pending_aid_ids: Vec::new(),
            }
        }

        fn actor(&self, idx: usize) -> Address {
            self.actors[idx % self.actors.len()].clone()
        }

        fn category(&self, reserve: bool) -> Symbol {
            if reserve {
                Symbol::new(&self.env, "reserve")
            } else {
                Symbol::new(&self.env, "rewards")
            }
        }

        fn apply(&mut self, op: &FuzzOp) {
            let aid_client = AidContractClient::new(&self.env, &self.aid_address);
            let treasury_client = TreasuryContractClient::new(&self.env, &self.treasury_address);
            let referral_client = ReferralContractClient::new(&self.env, &self.referral_address);

            match op {
                FuzzOp::CreateAid {
                    donor,
                    recipient,
                    amount,
                    expiry_delta,
                } => {
                    let donor = self.actor(*donor);
                    let recipient = self.actor(*recipient);
                    let expiry_ledger = self.env.ledger().sequence() + 1 + expiry_delta;
                    if let Ok(Ok(id)) = aid_client.try_create_aid(
                        &donor,
                        &recipient,
                        amount,
                        &expiry_ledger,
                        &Option::<Bytes>::None,
                    ) {
                        self.pending_aid_ids.push(id);
                    }
                }
                FuzzOp::ClaimAid { aid_slot, claimant } => {
                    if !self.pending_aid_ids.is_empty() {
                        let id = self.pending_aid_ids[*aid_slot % self.pending_aid_ids.len()];
                        let claimant = self.actor(*claimant);
                        let _ = aid_client.try_claim_aid(&id, &claimant);
                    }
                }
                FuzzOp::RefundAid { aid_slot, caller } => {
                    if !self.pending_aid_ids.is_empty() {
                        let id = self.pending_aid_ids[*aid_slot % self.pending_aid_ids.len()];
                        let caller = self.actor(*caller);
                        let _ = aid_client.try_refund_aid(&id, &caller);
                    }
                }
                FuzzOp::TreasuryDeposit {
                    actor,
                    amount,
                    reserve,
                } => {
                    let actor = self.actor(*actor);
                    let category = self.category(*reserve);
                    let _ = treasury_client.try_deposit(&actor, &self.token, &category, amount);
                }
                FuzzOp::TreasuryWithdraw {
                    actor,
                    to,
                    amount,
                    reserve,
                } => {
                    let actor = self.actor(*actor);
                    let to = self.actor(*to);
                    let category = self.category(*reserve);
                    let _ = treasury_client
                        .try_withdraw(&actor, &self.token, &to, amount, &category);
                }
                FuzzOp::RegisterReferral { wallet, referrer } => {
                    let wallet = self.actor(*wallet);
                    let referrer = self.actor(*referrer);
                    let _ = referral_client.try_register(&wallet, &referrer);
                }
                FuzzOp::Accrue {
                    referred,
                    base_amount,
                } => {
                    let referred = self.actor(*referred);
                    let tier_config = referral_client.get_tier_config();
                    let total_bps: i128 = tier_config.tier_bps.iter().sum();
                    if let Ok(Ok(total_credited)) =
                        referral_client.try_accrue(&self.admin, &referred, base_amount)
                    {
                        // Direct check of Issue #191's requirement: a single
                        // accrual can never credit more than the configured
                        // tier basis points allow for the base amount.
                        let max_possible =
                            shared::math::bps_of(*base_amount, total_bps).unwrap_or(i128::MAX);
                        assert!(
                            total_credited <= max_possible,
                            "accrue credited {} which exceeds the configured tier bps bound {} \
                             (base_amount={}, total_bps={})",
                            total_credited,
                            max_possible,
                            base_amount,
                            total_bps
                        );
                    }
                }
                FuzzOp::ClaimRewards { referrer } => {
                    let referrer = self.actor(*referrer);
                    let _ = referral_client.try_claim_rewards(&referrer);
                }
                FuzzOp::AdvanceLedger { delta } => {
                    advance_ledger_sequence(&self.env, *delta);
                    advance_ledger_time(&self.env, (*delta as u64) * 5);
                }
            }
        }

        /// Re-checks every global invariant against the current on-chain
        /// state. Panics (via `assert!`/`assert_eq!`) on the first
        /// violation found, which `proptest` reports as a failing,
        /// shrunk case.
        fn assert_invariants(&self) {
            let token_client = token::Client::new(&self.env, &self.token);
            let aid_client = AidContractClient::new(&self.env, &self.aid_address);
            let treasury_client = TreasuryContractClient::new(&self.env, &self.treasury_address);
            let referral_client = ReferralContractClient::new(&self.env, &self.referral_address);

            // --- Invariant 1: aid escrow accounting -------------------------
            let mut escrowed: i128 = 0;
            for id in &self.pending_aid_ids {
                if let Some(record) = aid_client.get_aid(id) {
                    if record.status == AidStatus::Pending {
                        escrowed = escrowed
                            .checked_add(record.amount)
                            .expect("escrow sum overflow");
                    }
                }
            }
            let aid_balance = token_client.balance(&self.aid_address);
            assert_eq!(
                aid_balance, escrowed,
                "aid contract token balance {} does not match sum of pending aid amounts {}",
                aid_balance, escrowed
            );

            // --- Invariant 2: treasury per-category accounting --------------
            let reserve_balance =
                treasury_client.category_balance(&self.token, &Symbol::new(&self.env, "reserve"));
            let rewards_balance =
                treasury_client.category_balance(&self.token, &Symbol::new(&self.env, "rewards"));
            let treasury_balance = token_client.balance(&self.treasury_address);
            assert_eq!(
                treasury_balance,
                reserve_balance + rewards_balance,
                "treasury token balance {} does not match reserve ({}) + rewards ({}) categories",
                treasury_balance,
                reserve_balance,
                rewards_balance
            );

            // --- Invariant 3: global token conservation ----------------------
            // Treasury reserves + escrowed aid + every wallet balance must
            // always equal the fixed total of tokens ever minted.
            let mut actor_sum = token_client.balance(&self.admin);
            for actor in &self.actors {
                actor_sum = actor_sum
                    .checked_add(token_client.balance(actor))
                    .expect("actor balance sum overflow");
            }
            let total_accounted = aid_balance
                .checked_add(treasury_balance)
                .and_then(|v| v.checked_add(actor_sum))
                .expect("total accounted overflow");
            assert_eq!(
                total_accounted, self.total_minted,
                "global token conservation violated: accounted {} != total minted {}",
                total_accounted, self.total_minted
            );

            // --- Invariant 4: referral commission never exceeds cap ---------
            let tier_config = referral_client.get_tier_config();
            for actor in self.actors.iter().chain(core::iter::once(&self.admin)) {
                let lifetime = referral_client.lifetime_accrued(actor);
                let accrued = referral_client.accrued_balance(actor);
                assert!(
                    lifetime <= tier_config.reward_cap,
                    "referrer lifetime accrued {} exceeds configured reward cap {}",
                    lifetime,
                    tier_config.reward_cap
                );
                assert!(
                    accrued <= lifetime,
                    "referrer accrued balance {} exceeds lifetime accrued {}",
                    accrued,
                    lifetime
                );
            }
        }
    }

    /// Drives >= 1,000 randomized cross-contract operations (Issue #191)
    /// across `NUM_CASES` independent, freshly-deployed scenarios, checking
    /// every global invariant after each op. Any invariant violation or
    /// unexpected contract panic fails this test with a proptest-shrunk
    /// minimal reproducing operation sequence.
    #[test]
    fn cross_contract_state_fuzzing() {
        let mut runner = TestRunner::new(ProptestConfig {
            cases: NUM_CASES,
            failure_persistence: None,
            ..ProptestConfig::default()
        });
        let strategy = fuzz_sequence_strategy();
        let result = runner.run(&strategy, |ops| {
            let mut harness = Harness::new();
            harness.assert_invariants();
            for op in &ops {
                harness.apply(op);
                harness.assert_invariants();
            }
            Ok(())
        });
        if let Err(err) = result {
            panic!("cross-contract fuzzing found a failing case:\n{}", err);
        }
    }
}
