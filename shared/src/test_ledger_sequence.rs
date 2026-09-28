//! Deterministic ledger-sequence edge-case scenarios (Issue #156).
//!
//! Every test here drives [`LedgerSequenceHarness`] instead of touching the
//! host clock: the ledger sequence and its derived timestamp are set
//! explicitly, and the escrow deadlines the tests assert on are the harness's
//! own [`LedgerSequenceHarness::deadline`] values. No wall clock, no
//! randomness, no `env.ledger().timestamp()` from the host.
//!
//! Scenario → boundary map from the acceptance criteria:
//!
//! | Scenario | Test | Boundary |
//! | --- | --- | --- |
//! | same-ledger | [`same_ledger_creation_and_release_are_deterministic`] | create and release in one ledger; a repeated release is `PaymentEscrowAlreadyReleased` |
//! | same-ledger | [`same_ledger_expiry_is_rejected`] | `expiry == current` is not "in the future" |
//! | next-ledger | [`next_ledger_release_exactly_at_expiry_is_allowed`] | release at `expiry` succeeds |
//! | expired-ledger | [`expired_ledger_is_rejected_then_refundable`] | release at `expiry + 1` is expired, refund is terminal |
//! | out-of-order | [`out_of_order_submission_cannot_rewind_the_ledger`] | a stale deadline/stamp is refused, never replayed |
//! | same/next/expired | [`aid_settlement_and_refund_boundaries_follow_the_ledger`] | the aid state machine flips exactly at `expiry + 1` |
//! | repeated submissions | [`repeated_submission_does_not_duplicate_release`] | two identical submissions produce one effect |
//! | repeated submissions | [`repeated_submission_with_a_different_payload_is_rejected`] | same key + different hash → `ConflictingRequest` |
//! | harness control | [`harness_clock_is_deterministic`] | sequence ⇒ timestamp, monotonic high-water mark |
//! | harness control | [`expiry_window_boundary_is_deterministic`] | the future-expiry window rule at its edges |

extern crate std;

use soroban_sdk::testutils::Address as _;
use soroban_sdk::{
    contract, contractimpl,
    token::{self, StellarAssetClient},
    Address, Bytes, BytesN, Env,
};

use crate::errors::Error;
use crate::idempotency::{self, IdempotencyError, Status};
use crate::ledger_sequence::{
    LedgerBoundary, LedgerControlError, LedgerOrder, LedgerSequenceHarness, SECONDS_PER_LEDGER,
};
use crate::lifecycle::{AidStateMachine, AidStatus, AidTransitionContext, StateMachine};
use crate::payments::{self, EscrowState};
use crate::semantic::{self, ExpiryRule};

/// TTL handed to the idempotency guard; long enough to outlive the scenario.
const IDEMPOTENCY_TTL: u32 = 1_000;

/// Ledger the fixtures start from (well above genesis so `expiry` arithmetic
/// stays in range).
const START_LEDGER: u32 = 20_000;

/// Contract boundary for the scenarios.
///
/// The harness needs a real contract frame: escrow records live in persistent
/// storage keyed off `current_contract_address()`, and the idempotency guard
/// uses temporary storage. The wrappers delegate to the **production**
/// `shared::payments` and `shared::idempotency` functions — only the
/// `require_auth` bookkeeping and the idempotency composition live here.
#[contract]
pub struct LedgerHarnessContract;

#[contractimpl]
impl LedgerHarnessContract {
    // -- Escrow, unmasked ---------------------------------------------------

    pub fn create_escrow(
        env: Env,
        token: Address,
        depositor: Address,
        beneficiary: Address,
        amount: i128,
        expiry_ledger: u32,
    ) -> Result<u64, Error> {
        depositor.require_auth();
        payments::create_escrow(&env, &token, &depositor, &beneficiary, amount, expiry_ledger)
    }

    pub fn release_escrow(env: Env, token: Address, escrow_id: u64) -> Result<(), Error> {
        payments::release_escrow(&env, &token, escrow_id)
    }

    pub fn refund_escrow(env: Env, token: Address, escrow_id: u64) -> Result<(), Error> {
        payments::refund_escrow(&env, &token, escrow_id)
    }

    pub fn get_escrow(env: Env, escrow_id: u64) -> Option<payments::EscrowRecord> {
        payments::get_escrow(&env, escrow_id)
    }

    // -- Escrow behind the idempotency guard --------------------------------

    /// Submit a release under `key`. The first submission performs the release
    /// and stores `escrow_id` as the replayable result; a retry carrying the
    /// same `request_hash` returns that stored result without executing the
    /// release again. A different payload under the same key subsumes into
    /// `Error::InvalidArgument` so the caller cannot tell a conflict from a
    /// rejected escrow.
    pub fn release_idempotent(
        env: Env,
        token: Address,
        key: Bytes,
        request_hash: BytesN<32>,
        escrow_id: u64,
    ) -> Result<u64, Error> {
        match idempotency::begin(&env, &key, &request_hash, IDEMPOTENCY_TTL) {
            Ok(Some(record)) => return Ok(record.result.unwrap_or(escrow_id)),
            Ok(None) => {}
            Err(_) => return Err(Error::InvalidArgument),
        }
        payments::release_escrow(&env, &token, escrow_id)?;
        idempotency::complete(&env, &key, &request_hash, escrow_id)
            .map_err(|_| Error::InvalidArgument)?;
        Ok(escrow_id)
    }

    /// Refund counterpart of [`Self::release_idempotent`].
    pub fn refund_idempotent(
        env: Env,
        token: Address,
        key: Bytes,
        request_hash: BytesN<32>,
        escrow_id: u64,
    ) -> Result<u64, Error> {
        match idempotency::begin(&env, &key, &request_hash, IDEMPOTENCY_TTL) {
            Ok(Some(record)) => return Ok(record.result.unwrap_or(escrow_id)),
            Ok(None) => {}
            Err(_) => return Err(Error::InvalidArgument),
        }
        payments::refund_escrow(&env, &token, escrow_id)?;
        idempotency::complete(&env, &key, &request_hash, escrow_id)
            .map_err(|_| Error::InvalidArgument)?;
        Ok(escrow_id)
    }
}

/// Depositor, beneficiary, token and contract client for one scenario.
struct Fixture<'a> {
    env: &'a Env,
    contract: LedgerHarnessContractClient<'a>,
    token: Address,
    token_client: token::Client<'a>,
    asset_client: StellarAssetClient<'a>,
    depositor: Address,
    beneficiary: Address,
}

impl<'a> Fixture<'a> {
    fn new(env: &'a Env) -> Self {
        env.mock_all_auths();
        let admin = Address::generate(env);
        let token = env.register_stellar_asset_contract(admin);
        let contract_id = env.register_contract(None, LedgerHarnessContract);
        Self {
            env,
            contract: LedgerHarnessContractClient::new(env, &contract_id),
            token_client: token::Client::new(env, &token),
            asset_client: StellarAssetClient::new(env, &token),
            token,
            depositor: Address::generate(env),
            beneficiary: Address::generate(env),
        }
    }

    fn fund(&self, amount: i128) {
        self.asset_client.mint(&self.depositor, &amount);
    }

    fn create(&self, amount: i128, expiry: u32) -> u64 {
        self.contract
            .create_escrow(&self.token, &self.depositor, &self.beneficiary, &amount, &expiry)
    }

    fn state(&self, escrow_id: u64) -> EscrowState {
        self.contract
            .get_escrow(&escrow_id)
            .expect("escrow record must exist")
            .state
    }

    fn contract_id(&self) -> Address {
        self.contract.address.clone()
    }

    fn hash(&self, byte: u8) -> BytesN<32> {
        BytesN::from_array(self.env, &[byte; 32])
    }

    fn key(&self, name: &str) -> Bytes {
        Bytes::from_slice(self.env, name.as_bytes())
    }
}

// ===========================================================================
// Harness controls
// ===========================================================================

#[test]
fn harness_clock_is_deterministic() {
    let env = Env::default();
    let mut harness = LedgerSequenceHarness::from_genesis(&env);

    assert_eq!(harness.sequence(), 1);
    assert_eq!(harness.timestamp(), 1_640_995_200);
    assert_eq!(harness.high_water(), Some(1));

    // Same-ledger submission: lands where it was submitted.
    assert_eq!(harness.order_of(1), LedgerOrder::SameLedger);
    // The next ledger is ahead of the high-water mark, not behind it.
    assert_eq!(harness.order_of(2), LedgerOrder::NextLedger);

    harness.advance(10);
    assert_eq!(harness.sequence(), 11);
    assert_eq!(harness.timestamp(), 1_640_995_200 + 10 * SECONDS_PER_LEDGER);
    assert_eq!(harness.high_water(), Some(11));

    // Boundary classification around a deadline of 21.
    assert_eq!(harness.classify(11, 11, 21), LedgerBoundary::SameLedger);
    assert_eq!(harness.classify(11, 12, 21), LedgerBoundary::NextLedger);
    assert_eq!(harness.classify(11, 21, 21), LedgerBoundary::NextLedger);
    assert_eq!(harness.classify(11, 22, 21), LedgerBoundary::Expired);
    // A landing ledger behind the stamp it carries is out of order — it was
    // composed for a newer ledger than the one it executed on.
    assert_eq!(harness.classify(11, 10, 21), LedgerBoundary::OutOfOrder);
    // A stamp behind the ledger already consumed is out of order even though
    // the execution itself would look like a same-ledger one.
    assert_eq!(harness.order_of(10), LedgerOrder::OutOfOrder);
    assert_eq!(harness.order_of(11), LedgerOrder::SameLedger);
    assert_eq!(harness.order_of(12), LedgerOrder::NextLedger);

    // The clock is forward-only in absolute mode, and it keeps the ledger
    // that has already been observed.
    assert_eq!(harness.at(50), Ok(()));
    assert_eq!(harness.sequence(), 50);
    assert_eq!(
        harness.at(49),
        Err(LedgerControlError::Backwards {
            requested: 49,
            current: 50
        })
    );
    assert_eq!(harness.sequence(), 50, "a refused rewind must not move the clock");
}

#[test]
fn expiry_window_boundary_is_deterministic() {
    let env = Env::default();
    let harness = LedgerSequenceHarness::from_state(&env, START_LEDGER);
    let rule = ExpiryRule {
        min_delay_ledgers: 5,
        max_delay_ledgers: 50,
    };

    let now = harness.sequence();
    assert_eq!(semantic::validate_future_expiry(&env, now + 5, &rule), Ok(()));
    assert_eq!(semantic::validate_future_expiry(&env, now + 50, &rule), Ok(()));
    assert_eq!(
        semantic::validate_future_expiry(&env, now + 4, &rule),
        Err(Error::InvalidArgument)
    );
    assert_eq!(
        semantic::validate_future_expiry(&env, now, &rule),
        Err(Error::InvalidArgument)
    );
    assert_eq!(
        semantic::validate_future_expiry(&env, now + 51, &rule),
        Err(Error::InvalidArgument)
    );
}

// ===========================================================================
// Same-ledger
// ===========================================================================

#[test]
fn same_ledger_creation_and_release_are_deterministic() {
    let env = Env::default();
    let mut harness = LedgerSequenceHarness::from_state(&env, START_LEDGER);
    let fixture = Fixture::new(&env);

    assert_eq!(harness.sequence(), START_LEDGER);
    assert_eq!(harness.classify(START_LEDGER, START_LEDGER, harness.deadline(20)), LedgerBoundary::SameLedger);

    fixture.fund(5_000);
    let escrow_id = fixture.create(2_000, harness.deadline(20));

    // Submitted and executed on the same ledger, so the release is in order.
    assert_eq!(harness.classify(START_LEDGER, harness.sequence(), harness.deadline(20)), LedgerBoundary::SameLedger);
    fixture.contract.release_escrow(&fixture.token, &escrow_id);
    assert_eq!(fixture.state(escrow_id), EscrowState::Released);
    assert_eq!(fixture.token_client.balance(&fixture.beneficiary), 2_000);
    assert_eq!(fixture.token_client.balance(&fixture.contract_id()), 0);

    // Repeating the release in the same ledger is refused: the effect is
    // already irreversibly applied, so it must not be applied twice.
    assert_eq!(
        fixture.contract.try_release_escrow(&fixture.token, &escrow_id),
        Err(Ok(Error::PaymentEscrowAlreadyReleased))
    );
    assert_eq!(fixture.token_client.balance(&fixture.beneficiary), 2_000);

    // And the ledger is untouched by both attempts.
    assert_eq!(harness.sequence(), START_LEDGER);
    assert_eq!(harness.high_water(), Some(START_LEDGER));
    harness.advance(0);
    assert_eq!(harness.classify(START_LEDGER, harness.sequence(), harness.deadline(20)), LedgerBoundary::SameLedger);
}

#[test]
fn same_ledger_expiry_is_rejected() {
    let env = Env::default();
    let harness = LedgerSequenceHarness::from_state(&env, START_LEDGER);
    let fixture = Fixture::new(&env);
    fixture.fund(5_000);

    // `expiry == current ledger` is not "in the future" — the create guard is
    // strictly greater, so a same-ledger deadline is rejected up front rather
    // than producing an escrow that can never be released.
    let now = harness.sequence();
    assert_eq!(harness.classify(now, now, now), LedgerBoundary::SameLedger);
    assert_eq!(
        fixture
            .contract
            .try_create_escrow(&fixture.token, &fixture.depositor, &fixture.beneficiary, &2_000, &now),
        Err(Ok(Error::InvalidArgument))
    );

    // The rejection left no escrow behind and moved no tokens.
    assert_eq!(fixture.contract.get_escrow(&1), None);
    assert_eq!(fixture.token_client.balance(&fixture.depositor), 5_000);
    assert_eq!(fixture.token_client.balance(&fixture.contract_id()), 0);
}

// ===========================================================================
// Next-ledger
// ===========================================================================

#[test]
fn next_ledger_release_exactly_at_expiry_is_allowed() {
    let env = Env::default();
    let mut harness = LedgerSequenceHarness::from_state(&env, START_LEDGER);
    let fixture = Fixture::new(&env);

    let expiry = harness.deadline(5);
    fixture.fund(5_000);
    let escrow_id = fixture.create(2_000, expiry);

    // Roll the clock to exactly the expiry ledger: the release window is
    // inclusive of the deadline.
    harness.advance(5);
    assert_eq!(harness.sequence(), expiry);
    assert_eq!(
        harness.classify(START_LEDGER, harness.sequence(), expiry),
        LedgerBoundary::NextLedger
    );

    fixture.contract.release_escrow(&fixture.token, &escrow_id);
    assert_eq!(fixture.state(escrow_id), EscrowState::Released);
    assert_eq!(fixture.token_client.balance(&fixture.beneficiary), 2_000);
    assert_eq!(fixture.token_client.balance(&fixture.contract_id()), 0);

    // The refund path is closed for the same escrow in the same ledger.
    assert_eq!(
        fixture.contract.try_refund_escrow(&fixture.token, &escrow_id),
        Err(Ok(Error::PaymentEscrowAlreadyReleased))
    );
}

// ===========================================================================
// Expired ledger
// ===========================================================================

#[test]
fn expired_ledger_is_rejected_then_refundable() {
    let env = Env::default();
    let mut harness = LedgerSequenceHarness::from_state(&env, START_LEDGER);
    let fixture = Fixture::new(&env);

    let expiry = harness.deadline(5);
    fixture.fund(5_000);
    let escrow_id = fixture.create(2_000, expiry);

    // One ledger past the deadline is the first expired ledger.
    harness.advance(6);
    assert_eq!(harness.sequence(), expiry + 1);
    assert_eq!(
        harness.classify(START_LEDGER, harness.sequence(), expiry),
        LedgerBoundary::Expired
    );

    assert_eq!(
        fixture.contract.try_release_escrow(&fixture.token, &escrow_id),
        Err(Ok(Error::PaymentEscrowExpired))
    );
    // A rejected release changes nothing.
    assert_eq!(fixture.state(escrow_id), EscrowState::Active);
    assert_eq!(fixture.token_client.balance(&fixture.contract_id()), 2_000);

    // Refund is the terminal transition on an expired escrow.
    fixture.contract.refund_escrow(&fixture.token, &escrow_id);
    assert_eq!(fixture.state(escrow_id), EscrowState::Refunded);
    assert_eq!(fixture.token_client.balance(&fixture.depositor), 5_000);
    assert_eq!(fixture.token_client.balance(&fixture.contract_id()), 0);

    // Refunded is irreversible, even much later.
    harness.advance(100);
    assert_eq!(
        fixture.contract.try_refund_escrow(&fixture.token, &escrow_id),
        Err(Ok(Error::PaymentEscrowAlreadyRefunded))
    );
    assert_eq!(
        fixture.contract.try_release_escrow(&fixture.token, &escrow_id),
        Err(Ok(Error::PaymentEscrowAlreadyRefunded))
    );
    assert_eq!(fixture.token_client.balance(&fixture.depositor), 5_000);
}

// ===========================================================================
// Aid lifecycle boundary (settle at expiry, refund strictly after)
// ===========================================================================

#[test]
fn aid_settlement_and_refund_boundaries_follow_the_ledger() {
    let env = Env::default();
    let mut harness = LedgerSequenceHarness::from_state(&env, START_LEDGER);
    let recipient = Address::generate(&env);
    let expiry = harness.deadline(5);

    let ctx = |current: u32| AidTransitionContext {
        current_ledger: current,
        expiry_ledger: expiry,
        caller: recipient.clone(),
        recipient: recipient.clone(),
    };

    // On the expiry ledger the claim still settles but the refund does not:
    // settlement is inclusive of the deadline.
    harness.advance(5);
    assert_eq!(harness.sequence(), expiry);
    assert_eq!(
        harness.classify(START_LEDGER, harness.sequence(), expiry),
        LedgerBoundary::NextLedger
    );
    assert_eq!(
        AidStateMachine::can_transition(
            AidStatus::Pending,
            AidStatus::Settled,
            &ctx(harness.sequence())
        ),
        Ok(())
    );
    assert_eq!(
        AidStateMachine::can_transition(
            AidStatus::Pending,
            AidStatus::Refunded,
            &ctx(harness.sequence())
        ),
        Err(Error::AidNotExpiredYet)
    );

    // One ledger later the two flip and stay flipped.
    harness.advance(1);
    assert_eq!(
        harness.classify(START_LEDGER, harness.sequence(), expiry),
        LedgerBoundary::Expired
    );
    assert_eq!(
        AidStateMachine::can_transition(
            AidStatus::Pending,
            AidStatus::Settled,
            &ctx(harness.sequence())
        ),
        Err(Error::Expired)
    );
    assert_eq!(
        AidStateMachine::can_transition(
            AidStatus::Pending,
            AidStatus::Refunded,
            &ctx(harness.sequence())
        ),
        Ok(())
    );

    // Terminal states are idempotent-rejected, so an expired claim can never be
    // settled by a later submission.
    harness.advance(1);
    assert_eq!(
        AidStateMachine::can_transition(
            AidStatus::Refunded,
            AidStatus::Settled,
            &ctx(harness.sequence())
        ),
        Err(Error::AidAlreadyRefunded)
    );
}

// ===========================================================================
// Out-of-order
// ===========================================================================

#[test]
fn out_of_order_submission_cannot_rewind_the_ledger() {
    let env = Env::default();
    let mut harness = LedgerSequenceHarness::from_state(&env, START_LEDGER);
    let fixture = Fixture::new(&env);
    fixture.fund(5_000);

    // A submission stamped before the ledger the harness has consumed is out
    // of order, and the control refuses to rewind the clock to "replay" it.
    let stale_ledger = harness.sequence();
    harness.advance(10);
    assert_eq!(harness.order_of(stale_ledger), LedgerOrder::OutOfOrder);
    // An execution that lands behind the stamp it carries is out of order: the
    // harness classifies it as such and refuses to manufacture that ordering
    // by moving the clock backwards.
    assert_eq!(
        harness.classify(stale_ledger + 10, stale_ledger, stale_ledger + 20),
        LedgerBoundary::OutOfOrder
    );
    assert_eq!(
        harness.at(stale_ledger),
        Err(LedgerControlError::Backwards {
            requested: stale_ledger,
            current: stale_ledger + 10
        })
    );
    assert_eq!(harness.sequence(), stale_ledger + 10);

    // A transaction composed at the stale ledger and executed now is judged
    // against the *current* ledger: its deadline has already passed, so it is
    // rejected instead of minting an escrow that is born expired.
    assert_eq!(
        fixture
            .contract
            .try_create_escrow(&fixture.token, &fixture.depositor, &fixture.beneficiary, &2_000, &(stale_ledger + 1)),
        Err(Ok(Error::InvalidArgument))
    );

    // The same payload executed a ledger later is fine because the deadline is
    // still ahead of the current ledger — out-of-order-ness, not the deadline,
    // is what made the previous submission invalid.
    let valid_expiry = harness.deadline(5);
    let escrow_id = fixture.create(2_000, valid_expiry);
    assert_eq!(fixture.state(escrow_id), EscrowState::Active);
    assert_eq!(harness.classify(harness.sequence(), harness.sequence(), valid_expiry), LedgerBoundary::SameLedger);
}

// ===========================================================================
// Repeated submissions
// ===========================================================================

#[test]
fn repeated_submission_does_not_duplicate_release() {
    let env = Env::default();
    let mut harness = LedgerSequenceHarness::from_state(&env, START_LEDGER);
    let fixture = Fixture::new(&env);

    let expiry = harness.deadline(10);
    fixture.fund(5_000);
    let escrow_id = fixture.create(2_000, expiry);

    let key = fixture.key("release-1");
    let hash = fixture.hash(7);

    // First submission: the release happens and the result is stored.
    assert_eq!(
        fixture
            .contract
            .release_idempotent(&fixture.token, &key, &hash, &escrow_id),
        escrow_id
    );
    assert_eq!(fixture.state(escrow_id), EscrowState::Released);
    assert_eq!(fixture.token_client.balance(&fixture.beneficiary), 2_000);

    // Second submission in the same ledger: a replay, so the release is not
    // applied again and the same result comes back.
    assert_eq!(
        fixture
            .contract
            .release_idempotent(&fixture.token, &key, &hash, &escrow_id),
        escrow_id
    );
    assert_eq!(fixture.token_client.balance(&fixture.beneficiary), 2_000);
    assert_eq!(fixture.token_client.balance(&fixture.contract_id()), 0);

    // Still one effect after the ledger progresses, and the underlying call
    // would reject a third attempt.
    harness.advance(1);
    assert_eq!(
        fixture
            .contract
            .release_idempotent(&fixture.token, &key, &hash, &escrow_id),
        escrow_id
    );
    assert_eq!(fixture.token_client.balance(&fixture.beneficiary), 2_000);
    assert_eq!(
        fixture.contract.try_release_escrow(&fixture.token, &escrow_id),
        Err(Ok(Error::PaymentEscrowAlreadyReleased))
    );
    assert_eq!(harness.sequence(), START_LEDGER + 1);
    assert!(harness.sequence() <= expiry);
}

#[test]
fn repeated_submission_with_a_different_payload_is_rejected() {
    let env = Env::default();
    let fixture = Fixture::new(&env);
    let key = fixture.key("release-2");
    let first = fixture.hash(1);
    let second = fixture.hash(2);

    let contract_id = fixture.contract_id();
    env.as_contract(&contract_id, || {
        // First submission reserves the key.
        assert_eq!(idempotency::begin(&env, &key, &first, IDEMPOTENCY_TTL), Ok(None));
        assert_eq!(
            idempotency::begin(&env, &key, &first, IDEMPOTENCY_TTL),
            Err(IdempotencyError::InProgress)
        );
        assert_eq!(idempotency::complete(&env, &key, &first, 42), Ok(()));

        // Same key + same hash → replayed completed record, no second effect.
        let replay = idempotency::begin(&env, &key, &first, IDEMPOTENCY_TTL)
            .expect("completed request must replay");
        let replay = replay.expect("a completed record is returned on retry");
        assert_eq!(replay.status, Status::Completed);
        assert_eq!(replay.result, Some(42));

        // Completing twice is refused.
        assert_eq!(
            idempotency::complete(&env, &key, &first, 42),
            Err(IdempotencyError::AlreadyCompleted)
        );
        assert_eq!(
            idempotency::complete(&env, &key, &second, 42),
            Err(IdempotencyError::ConflictingRequest)
        );

        // Same key + different hash → conflict, never a replay.
        assert_eq!(
            idempotency::begin(&env, &key, &second, IDEMPOTENCY_TTL),
            Err(IdempotencyError::ConflictingRequest)
        );
    });
}
