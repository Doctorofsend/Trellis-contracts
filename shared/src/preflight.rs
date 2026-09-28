//! Deterministic preflight for high-risk operations (Issue #109).
//!
//! High-risk Trellis operations — treasury withdrawals, escrow settlement,
//! contract upgrades, migrations, rebalances and batch invocations — should be
//! checked *before* the caller signs or submits a transaction. Today the
//! workspace has the pieces ([`crate::policy`] evaluates business rules,
//! [`crate::semantic`] validates individual values, [`crate::error_taxonomy`]
//! renders safe messages after the fact) but nothing that runs them together
//! and returns a single actionable verdict. [`preflight`] is that entry point.
//!
//! ## Status taxonomy
//!
//! Every check resolves to one of three [`PreflightStatus`] values:
//!
//! | Status | Meaning | Caller action |
//! |---|---|---|
//! | `Ready` | No findings. | Submit. |
//! | `Warning` | Submittable, but the caller should read the warning and remediation. | Submit after acknowledging, or fix the flagged risk first. |
//! | `Blocked` | Known-invalid; submission will fail or is unsafe. | Do not submit — follow the remediation and re-run preflight. |
//!
//! A report's overall status is `Blocked` if any check is `Blocked`, otherwise
//! `Warning` if any check is `Warning`, otherwise `Ready`. The
//! [`PreflightReport::primary_code`] is the first `Blocked` code, or the first
//! `Warning` code when nothing is blocked.
//!
//! ## Determinism
//!
//! [`preflight`] is a pure function of its [`PreflightInput`]: it never reads
//! the ledger, storage, or a clock. Ledger sequence, timestamps and the
//! observed state version are supplied by the caller, so the same input always
//! produces an equal [`PreflightReport`] — in tests and on-chain. The
//! [`require_preflight`] guard maps a blocking report onto the existing shared
//! [`Error`] codes, exactly like [`crate::policy::require_policy`], so contract
//! entry points keep returning stable, user-safe errors.
//!
//! ```ignore
//! use shared::preflight::{preflight, require_preflight, PreflightInput, PreflightOperation};
//!
//! # let env = soroban_sdk::Env::default();
//! let input = PreflightInput { /* typed facts, no env reads */ .. };
//! let report = preflight(&env, &input);
//! if report.is_blocked() {
//!     // surface report.checks[..].message + remediation to the caller
//! }
//! require_preflight(&env, &input)?; // Err(Error) when blocked
//! ```

use soroban_sdk::{contracttype, Env, String, Vec};

use crate::error_taxonomy::ErrorCategory;
use crate::errors::Error;
use crate::policy::{evaluate, validate_policy, PolicyConfig, PolicyInput, PolicyReason};

/// Share of the configured maximum amount at which a preflight warns that an
/// operation is high-value and warrants a second look (80%).
pub const HIGH_VALUE_WARNING_BPS: u32 = 8_000;

/// How close (in ledgers) an expiry may be before preflight warns instead of
/// passing silently.
pub const EXPIRY_WARNING_WINDOW_LEDGERS: u32 = 100;

/// How much scrutiny an operation needs before submission.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RiskLevel {
    /// No preflight required; ordinary validated entry point.
    Standard,
    /// Multi-step or settlement-adjacent; preflight recommended.
    Elevated,
    /// Moves funds, code, or storage irreversibly; preflight required.
    High,
}

/// The operations that can be preflighted, keyed by the contract family and
/// intent rather than by a specific entry-point name.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreflightOperation {
    /// Treasury withdrawal of held funds.
    TreasuryWithdraw,
    /// Escrow release to the beneficiary.
    EscrowRelease,
    /// Escrow refund to the depositor.
    EscrowRefund,
    /// Applying a pending WASM upgrade with a migration hook.
    UpgradeExecute,
    /// Resuming a partially applied storage migration.
    MigrationResume,
    /// Rebalancer strategy execution.
    RebalanceExecute,
    /// Multi-operation batch invocation.
    BatchInvoke,
    /// Aid claim/settlement.
    AidSettle,
    /// Plain single-recipient transfer.
    Transfer,
}

impl PreflightOperation {
    /// The scrutiny this operation needs.
    pub fn risk_level(&self) -> RiskLevel {
        match self {
            Self::TreasuryWithdraw
            | Self::EscrowRelease
            | Self::EscrowRefund
            | Self::UpgradeExecute
            | Self::MigrationResume
            | Self::RebalanceExecute => RiskLevel::High,
            Self::BatchInvoke | Self::AidSettle => RiskLevel::Elevated,
            Self::Transfer => RiskLevel::Standard,
        }
    }

    /// `true` when the operation must run preflight before submission.
    pub fn requires_preflight(&self) -> bool {
        !matches!(self.risk_level(), RiskLevel::Standard)
    }
}

/// Convenience wrapper for [`PreflightOperation::requires_preflight`].
pub fn requires_preflight(operation: &PreflightOperation) -> bool {
    operation.requires_preflight()
}

/// Verdict for a single check or for the whole report.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreflightStatus {
    Ready,
    Warning,
    Blocked,
}

/// Machine-readable reason a check fired. Stable for clients.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreflightCode {
    /// No findings.
    Ok,
    /// The contract is paused.
    ContractPaused,
    /// The caller's expected state version no longer matches the observed one.
    StaleState,
    /// The operation's expiry window has passed.
    Expired,
    /// The expiry window is about to close.
    ExpirySoon,
    /// Amount is below the configured minimum.
    AmountBelowMinimum,
    /// Amount is above the configured maximum.
    AmountAboveMaximum,
    /// Actor tier is not permitted.
    TierNotAllowed,
    /// Region is not permitted.
    RegionRestricted,
    /// The daily operation cap has been reached.
    DailyLimitReached,
    /// One daily operation slot remains.
    ApproachingDailyLimit,
    /// Amount is in the top of the configured range.
    HighValue,
    /// The policy configuration itself is invalid.
    InvalidConfig,
}

/// One actionable preflight finding: a user-safe explanation plus, for
/// warnings and blocks, a remediation step.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreflightCheck {
    pub code: PreflightCode,
    pub status: PreflightStatus,
    /// Client-facing category, reusing [`ErrorCategory`] so callers can pick
    /// safe retry/recovery behavior with one mapping.
    pub category: ErrorCategory,
    /// User-safe explanation. Never contains internal state or secrets.
    pub message: String,
    /// What the caller should do next. Present for `Warning`/`Blocked`.
    pub remediation: Option<String>,
}

/// Typed facts a caller supplies to [`preflight`]. All ledger/time/state
/// values are explicit inputs, which is what makes the result deterministic.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreflightInput {
    pub operation: PreflightOperation,
    pub amount: i128,
    pub actor_tier: u32,
    pub daily_ops: u32,
    pub region_allowed: bool,
    pub config: PolicyConfig,
    pub paused: bool,
    /// The state version the caller based this transaction on.
    pub expected_state_version: u32,
    /// The state version currently observed on-chain.
    pub observed_state_version: u32,
    pub current_ledger: u32,
    /// Expiry ledger for the operation; `0` means it does not expire.
    pub expiry_ledger: u32,
}

/// The result of running [`preflight`]. See the module docs for the status
/// taxonomy and how `primary_code` is chosen.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreflightReport {
    pub status: PreflightStatus,
    pub risk_level: RiskLevel,
    pub primary_code: PreflightCode,
    pub checks: Vec<PreflightCheck>,
}

impl PreflightReport {
    pub fn is_ready(&self) -> bool {
        self.status == PreflightStatus::Ready
    }

    pub fn is_blocked(&self) -> bool {
        self.status == PreflightStatus::Blocked
    }

    pub fn is_warning(&self) -> bool {
        self.status == PreflightStatus::Warning
    }

    /// All blocking checks, in the deterministic order they were evaluated.
    pub fn blocking_checks(&self) -> Vec<PreflightCheck> {
        self.filter_status(PreflightStatus::Blocked)
    }

    /// All warning checks, in the deterministic order they were evaluated.
    pub fn warnings(&self) -> Vec<PreflightCheck> {
        self.filter_status(PreflightStatus::Warning)
    }

    /// The check with `code`, if it fired.
    pub fn check(&self, code: PreflightCode) -> Option<PreflightCheck> {
        self.checks.iter().find(|c| c.code == code)
    }

    fn filter_status(&self, want: PreflightStatus) -> Vec<PreflightCheck> {
        let mut out = Vec::new(self.checks.env());
        for c in self.checks.iter() {
            if c.status == want {
                out.push_back(c.clone());
            }
        }
        out
    }
}

fn check(
    env: &Env,
    code: PreflightCode,
    status: PreflightStatus,
    category: ErrorCategory,
    message: &str,
    remediation: Option<&str>,
) -> PreflightCheck {
    PreflightCheck {
        code,
        status,
        category,
        message: String::from_str(env, message),
        remediation: remediation.map(|text| String::from_str(env, text)),
    }
}

/// Run every preflight check for `input` and combine them into one report.
///
/// Checks run in a fixed order — pause, state version, expiry, configuration,
/// policy, then the advisory (non-blocking) checks — so reports are stable and
/// reproducible. Pure: no ledger, storage, or clock reads.
pub fn preflight(env: &Env, input: &PreflightInput) -> PreflightReport {
    let mut checks = Vec::new(env);

    if input.paused {
        checks.push_back(check(
            env,
            PreflightCode::ContractPaused,
            PreflightStatus::Blocked,
            ErrorCategory::Conflict,
            "This action is temporarily unavailable while the service is paused.",
            Some("Try again after the service has resumed."),
        ));
    }

    if input.expected_state_version != input.observed_state_version {
        checks.push_back(check(
            env,
            PreflightCode::StaleState,
            PreflightStatus::Blocked,
            ErrorCategory::Conflict,
            "The item changed since this transaction was prepared.",
            Some("Refresh the item, re-run preflight, then submit the refreshed transaction."),
        ));
    }

    if input.expiry_ledger != 0 {
        if input.current_ledger >= input.expiry_ledger {
            checks.push_back(check(
                env,
                PreflightCode::Expired,
                PreflightStatus::Blocked,
                ErrorCategory::Conflict,
                "This request has expired and can no longer be submitted.",
                Some("Create a new request with a fresh expiry before retrying."),
            ));
        } else if input.expiry_ledger - input.current_ledger <= EXPIRY_WARNING_WINDOW_LEDGERS {
            checks.push_back(check(
                env,
                PreflightCode::ExpirySoon,
                PreflightStatus::Warning,
                ErrorCategory::Conflict,
                "This request expires soon.",
                Some("Submit before the expiry ledger, or refresh the request for a new window."),
            ));
        }
    }

    if validate_policy(&input.config).is_err() {
        checks.push_back(check(
            env,
            PreflightCode::InvalidConfig,
            PreflightStatus::Blocked,
            ErrorCategory::Configuration,
            "The service's operation rules are unavailable, so this request cannot be checked.",
            Some("Contact a maintainer and provide the reference ID."),
        ));
    } else {
        let decision = evaluate(
            &input.config,
            &PolicyInput {
                amount: input.amount,
                actor_tier: input.actor_tier,
                daily_ops: input.daily_ops,
                region_allowed: input.region_allowed,
            },
        );

        match decision.reason {
            PolicyReason::Allowed => {}
            PolicyReason::AmountBelowMinimum => checks.push_back(check(
                env,
                PreflightCode::AmountBelowMinimum,
                PreflightStatus::Blocked,
                ErrorCategory::Validation,
                "The amount is below the minimum allowed for this action.",
                Some("Increase the amount to at least the published minimum and try again."),
            )),
            PolicyReason::AmountAboveMaximum => checks.push_back(check(
                env,
                PreflightCode::AmountAboveMaximum,
                PreflightStatus::Blocked,
                ErrorCategory::Validation,
                "The amount is above the maximum allowed for this action.",
                Some("Reduce the amount to within the published limit, or split it into smaller operations."),
            )),
            PolicyReason::TierNotAllowed => checks.push_back(check(
                env,
                PreflightCode::TierNotAllowed,
                PreflightStatus::Blocked,
                ErrorCategory::Authorization,
                "Your account tier is not eligible for this action.",
                Some("Complete the required verification or use an eligible account."),
            )),
            PolicyReason::RegionRestricted => checks.push_back(check(
                env,
                PreflightCode::RegionRestricted,
                PreflightStatus::Blocked,
                ErrorCategory::Authorization,
                "This action is not available in your region.",
                Some("Contact a maintainer if you believe your region should be supported."),
            )),
            PolicyReason::DailyLimitReached => checks.push_back(check(
                env,
                PreflightCode::DailyLimitReached,
                PreflightStatus::Blocked,
                ErrorCategory::Conflict,
                "You have reached the daily limit for this action.",
                Some("Wait for the daily limit window to reset, then try again."),
            )),
            PolicyReason::InvalidConfig => checks.push_back(check(
                env,
                PreflightCode::InvalidConfig,
                PreflightStatus::Blocked,
                ErrorCategory::Configuration,
                "The service's operation rules are unavailable, so this request cannot be checked.",
                Some("Contact a maintainer and provide the reference ID."),
            )),
        }

        if decision.allowed {
            if input.daily_ops.saturating_add(1) >= input.config.max_daily_ops {
                checks.push_back(check(
                    env,
                    PreflightCode::ApproachingDailyLimit,
                    PreflightStatus::Warning,
                    ErrorCategory::Conflict,
                    "This is your last operation for the current daily limit window.",
                    Some("Submit now if it is still needed, or retry after the window resets."),
                ));
            }

            let threshold = input
                .config
                .max_amount
                .saturating_mul(HIGH_VALUE_WARNING_BPS as i128)
                / 10_000;
            if threshold > 0 && input.amount >= threshold {
                checks.push_back(check(
                    env,
                    PreflightCode::HighValue,
                    PreflightStatus::Warning,
                    ErrorCategory::Settlement,
                    "This is a high-value operation relative to the configured limit.",
                    Some("Confirm the amount and recipient before submitting."),
                ));
            }
        }
    }

    let (status, primary_code) = combine(&checks);
    PreflightReport {
        status,
        risk_level: input.operation.risk_level(),
        primary_code,
        checks,
    }
}

/// Guard for contract entry points: `Ok(report)` when submittable (including
/// warnings), otherwise the shared [`Error`] that matches the primary block.
///
/// Warnings are returned rather than discarded so the caller can surface the
/// explanation and remediation through [`crate::disclosure`].
pub fn require_preflight(env: &Env, input: &PreflightInput) -> Result<PreflightReport, Error> {
    let report = preflight(env, input);
    if report.status != PreflightStatus::Blocked {
        return Ok(report);
    }
    Err(match report.primary_code {
        PreflightCode::ContractPaused => Error::ContractPaused,
        PreflightCode::StaleState => Error::StaleData,
        PreflightCode::Expired => Error::Expired,
        PreflightCode::AmountBelowMinimum | PreflightCode::AmountAboveMaximum => {
            Error::InvalidAmount
        }
        PreflightCode::TierNotAllowed | PreflightCode::RegionRestricted => Error::Unauthorized,
        PreflightCode::DailyLimitReached => Error::WithdrawalLimitExceeded,
        PreflightCode::InvalidConfig => Error::ConfigInvalid,
        // `require_preflight` only errors on `Blocked`, and every current
        // blocking code is handled above; fail closed if a future code is added.
        _ => Error::InvalidArgument,
    })
}

fn combine(checks: &Vec<PreflightCheck>) -> (PreflightStatus, PreflightCode) {
    let mut status = PreflightStatus::Ready;
    let mut primary = PreflightCode::Ok;

    for c in checks.iter() {
        match c.status {
            PreflightStatus::Blocked => {
                if status != PreflightStatus::Blocked {
                    status = PreflightStatus::Blocked;
                    primary = c.code;
                }
            }
            PreflightStatus::Warning => {
                if status == PreflightStatus::Ready {
                    status = PreflightStatus::Warning;
                    primary = c.code;
                }
            }
            PreflightStatus::Ready => {}
        }
    }

    (status, primary)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> PolicyConfig {
        PolicyConfig {
            min_amount: 1,
            max_amount: 1_000_000,
            max_daily_ops: 10,
            allowed_max_tier: 3,
            region_allowed: true,
        }
    }

    fn input(operation: PreflightOperation) -> PreflightInput {
        PreflightInput {
            operation,
            amount: 100,
            actor_tier: 1,
            daily_ops: 0,
            region_allowed: true,
            config: config(),
            paused: false,
            expected_state_version: 7,
            observed_state_version: 7,
            current_ledger: 1_000,
            expiry_ledger: 2_000,
        }
    }

    #[test]
    fn high_risk_operations_are_classified() {
        assert_eq!(
            PreflightOperation::TreasuryWithdraw.risk_level(),
            RiskLevel::High
        );
        assert_eq!(
            PreflightOperation::UpgradeExecute.risk_level(),
            RiskLevel::High
        );
        assert_eq!(
            PreflightOperation::BatchInvoke.risk_level(),
            RiskLevel::Elevated
        );
        assert_eq!(
            PreflightOperation::Transfer.risk_level(),
            RiskLevel::Standard
        );

        assert!(requires_preflight(&PreflightOperation::TreasuryWithdraw));
        assert!(requires_preflight(&PreflightOperation::MigrationResume));
        assert!(!requires_preflight(&PreflightOperation::Transfer));
    }

    #[test]
    fn successful_preflight_is_ready_with_no_findings() {
        let env = Env::default();
        let report = preflight(&env, &input(PreflightOperation::TreasuryWithdraw));

        assert!(report.is_ready());
        assert!(!report.is_blocked());
        assert_eq!(report.status, PreflightStatus::Ready);
        assert_eq!(report.primary_code, PreflightCode::Ok);
        assert_eq!(report.risk_level, RiskLevel::High);
        assert!(report.checks.is_empty());
        assert!(report.warnings().is_empty());
        assert!(report.blocking_checks().is_empty());
    }

    #[test]
    fn high_value_operation_warns_with_remediation() {
        let env = Env::default();
        let mut i = input(PreflightOperation::TreasuryWithdraw);
        i.amount = i.config.max_amount; // == 100% of the configured maximum

        let report = preflight(&env, &i);
        assert!(report.is_warning());
        assert_eq!(report.primary_code, PreflightCode::HighValue);

        let warning = report.check(PreflightCode::HighValue).unwrap();
        assert_eq!(warning.status, PreflightStatus::Warning);
        assert_eq!(warning.category, ErrorCategory::Settlement);
        assert!(!warning.message.is_empty());
        assert!(warning.remediation.is_some());
        assert_eq!(report.warnings().len(), 1);
    }

    #[test]
    fn last_daily_slot_warns_before_it_blocks() {
        let env = Env::default();
        let mut i = input(PreflightOperation::RebalanceExecute);
        i.daily_ops = i.config.max_daily_ops - 1;

        let report = preflight(&env, &i);
        assert!(report.is_warning());
        assert_eq!(report.primary_code, PreflightCode::ApproachingDailyLimit);

        // The warning fires one slot before `policy` would deny.
        i.daily_ops = i.config.max_daily_ops;
        let blocked = preflight(&env, &i);
        assert!(blocked.is_blocked());
        assert_eq!(blocked.primary_code, PreflightCode::DailyLimitReached);
    }

    #[test]
    fn policy_violations_block_before_submission() {
        let env = Env::default();
        let cfg = config();

        let mut below = input(PreflightOperation::TreasuryWithdraw);
        below.amount = 0;
        let report = preflight(&env, &below);
        assert!(report.is_blocked());
        assert_eq!(report.primary_code, PreflightCode::AmountBelowMinimum);

        let mut above = input(PreflightOperation::TreasuryWithdraw);
        above.amount = cfg.max_amount + 1;
        let report = preflight(&env, &above);
        assert!(report.is_blocked());
        assert_eq!(report.primary_code, PreflightCode::AmountAboveMaximum);

        let mut tier = input(PreflightOperation::TreasuryWithdraw);
        tier.actor_tier = cfg.allowed_max_tier + 1;
        let report = preflight(&env, &tier);
        assert!(report.is_blocked());
        assert_eq!(report.primary_code, PreflightCode::TierNotAllowed);
        assert_eq!(
            report
                .check(PreflightCode::TierNotAllowed)
                .unwrap()
                .category,
            ErrorCategory::Authorization
        );
    }

    #[test]
    fn stale_state_version_blocks_with_refresh_guidance() {
        let env = Env::default();
        let mut i = input(PreflightOperation::EscrowRelease);
        i.expected_state_version = 7;
        i.observed_state_version = 9;

        let report = preflight(&env, &i);
        assert!(report.is_blocked());
        assert_eq!(report.primary_code, PreflightCode::StaleState);

        let stale = report.check(PreflightCode::StaleState).unwrap();
        assert_eq!(stale.category, ErrorCategory::Conflict);
        assert!(stale.remediation.is_some());
    }

    #[test]
    fn expiry_blocks_when_passed_and_warns_when_close() {
        let env = Env::default();

        let mut expired = input(PreflightOperation::EscrowRelease);
        expired.current_ledger = expired.expiry_ledger;
        let report = preflight(&env, &expired);
        assert!(report.is_blocked());
        assert_eq!(report.primary_code, PreflightCode::Expired);

        let mut soon = input(PreflightOperation::EscrowRelease);
        soon.current_ledger = soon.expiry_ledger - EXPIRY_WARNING_WINDOW_LEDGERS;
        let report = preflight(&env, &soon);
        assert!(report.is_warning());
        assert_eq!(report.primary_code, PreflightCode::ExpirySoon);

        // No expiry window is not a finding.
        let mut no_expiry = input(PreflightOperation::EscrowRelease);
        no_expiry.expiry_ledger = 0;
        assert!(preflight(&env, &no_expiry).is_ready());
    }

    #[test]
    fn paused_and_invalid_config_fail_closed() {
        let env = Env::default();

        let mut paused = input(PreflightOperation::TreasuryWithdraw);
        paused.paused = true;
        let report = preflight(&env, &paused);
        assert!(report.is_blocked());
        assert_eq!(report.primary_code, PreflightCode::ContractPaused);

        let mut bad = input(PreflightOperation::TreasuryWithdraw);
        bad.config.min_amount = 0;
        let report = preflight(&env, &bad);
        assert!(report.is_blocked());
        assert_eq!(report.primary_code, PreflightCode::InvalidConfig);
    }

    #[test]
    fn blocked_takes_precedence_over_warnings() {
        let env = Env::default();
        let mut i = input(PreflightOperation::TreasuryWithdraw);
        i.paused = true;
        i.amount = i.config.max_amount; // would otherwise warn HighValue

        let report = preflight(&env, &i);
        assert!(report.is_blocked());
        assert_eq!(report.primary_code, PreflightCode::ContractPaused);
        assert_eq!(report.blocking_checks().len(), 1);
        // The advisory warning is preserved for disclosure, just not primary.
        assert!(report.check(PreflightCode::HighValue).is_some());
    }

    #[test]
    fn require_preflight_maps_blocks_to_shared_errors() {
        let env = Env::default();

        let mut stale = input(PreflightOperation::EscrowRelease);
        stale.observed_state_version = 8;
        assert_eq!(require_preflight(&env, &stale), Err(Error::StaleData));

        let mut paused = input(PreflightOperation::TreasuryWithdraw);
        paused.paused = true;
        assert_eq!(require_preflight(&env, &paused), Err(Error::ContractPaused));

        let mut above = input(PreflightOperation::TreasuryWithdraw);
        above.amount = above.config.max_amount + 1;
        assert_eq!(require_preflight(&env, &above), Err(Error::InvalidAmount));

        // Warnings are submittable and returned to the caller.
        let mut warn = input(PreflightOperation::TreasuryWithdraw);
        warn.amount = warn.config.max_amount;
        let report = require_preflight(&env, &warn).unwrap();
        assert!(report.is_warning());
    }

    #[test]
    fn preflight_is_deterministic_for_equal_inputs() {
        let env = Env::default();
        let i = input(PreflightOperation::MigrationResume);

        let first = preflight(&env, &i);
        let second = preflight(&env, &i);

        assert_eq!(first, second);
        assert_eq!(first.checks.len(), second.checks.len());
    }
}
