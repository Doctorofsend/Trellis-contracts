#![no_std]
//! # Treasury Contract
//!
//! Protocol treasury management for Trellis humanitarian aid platform.
//!
//! ## Overview
//!
//! The Treasury Contract is the financial heart of Trellis, managing:
//! - **Multi-token per-category balances** (e.g., `reserve`, `rewards`) for fund segregation
//! - **Withdrawal limits** to prevent accidental large transfers
//! - **Role-based access control** via treasury managers and administrators
//! - **Referral reward distribution** through integration with the referral contract
//!
//! ## Categories
//!
//! The treasury organizes funds into categories per asset, each with its own balance:
//! - **`reserve`**: Protocol emergency funds
//! - **`rewards`**: Referral commission pool
//! - Custom categories as defined by administrators
//!
//! ## Roles
//!
//! - **Admin**: Full governance (set managers, configure limits, emergency withdraw)
//! - **Treasury Manager**: Operational access (deposit, withdraw, distribute rewards)
//!
//! ## Queries
//!
//! - [`TreasuryContract::category_balance`]: Check an asset category's current balance
//! - [`TreasuryContract::withdrawal_limit`]: View the max per-transaction limit
//! - [`TreasuryContract::referral_contract`]: See the registered referral contract

use soroban_sdk::{contract, contractimpl, symbol_short, Address, Bytes, Env, Symbol};

use shared::auth::{self, Permission, Role};
use shared::errors::Error;
use shared::events::{
    self, emit_action_executed, emit_commission_paid, emit_module_initialized,
    emit_permission_changed, emit_treasury_deposit, emit_treasury_withdrawal,
};
use shared::storage::{instance_get, instance_set, persistent_set};
use shared::{record_action_audit_event, ResourceLink, TimelineEventType};

/// Storage key prefix for per-category multi-token balances; the full key is
/// `(BALANCE, token, category)`.
const BALANCE: Symbol = symbol_short!("cat_bal");
/// Storage key for the configurable max per-transaction withdrawal limit.
const MAX_WD: Symbol = symbol_short!("max_wd");
/// Category symbol for the emergency reserve (used by `emergency_withdraw`).
const RESERVE_CATEGORY: Symbol = symbol_short!("reserve");
/// Category symbol for the referral commission rewards pool (used by
/// `distribute_reward`).
const REWARDS_CATEGORY: Symbol = symbol_short!("rewards");
/// Storage key for the referral contract address authorised to call
/// `distribute_reward`.
const REFERRAL_CONTRACT: Symbol = symbol_short!("ref_ctr");

fn record_treasury_audit(
    env: &Env,
    actor: &Address,
    event_type: TimelineEventType,
    action: Symbol,
    reason: Symbol,
    resource: Option<Address>,
    attribute: Option<Symbol>,
    before: Option<i128>,
    after: Option<i128>,
) -> Result<(), Error> {
    record_action_audit_event(
        env,
        actor,
        event_type,
        ResourceLink {
            kind: Bytes::from_slice(env, b"treasury"),
            id: 0,
            revision: 0,
        },
        symbol_short!("treasury"),
        action,
        reason,
        resource,
        attribute,
        before,
        after,
    )?;
    Ok(())
}

#[contract]
pub struct TreasuryContract;

#[contractimpl]
impl TreasuryContract {
    /// Initialise the contract: sets the admin address, grants the admin
    /// the `TreasuryManager` role, and sets the initial max
    /// per-transaction withdrawal limit.
    pub fn initialize(env: Env, admin: Address, max_withdrawal_limit: i128) -> Result<(), Error> {
        if max_withdrawal_limit <= 0 {
            return Err(Error::InvalidArgument);
        }
        auth::initialize_admin(&env, &admin)?;
        persistent_set(
            &env,
            &shared::auth::DataKey::Role(admin.clone(), Role::TreasuryManager),
            &true,
        );
        instance_set(&env, &MAX_WD, &max_withdrawal_limit);
        record_treasury_audit(
            &env,
            &admin,
            TimelineEventType::ConfigChanged,
            symbol_short!("init"),
            symbol_short!("setup"),
            None,
            None,
            None,
            Some(max_withdrawal_limit),
        )?;
        emit_module_initialized(
            &env,
            symbol_short!("treasury"),
            1,
            &admin,
            env.ledger().timestamp(),
        );
        Ok(())
    }

    /// Grants the `TreasuryManager` role to `who`. Admin only.
    pub fn add_treasury_manager(env: Env, caller: Address, who: Address) -> Result<(), Error> {
        let was_manager = auth::has_role(&env, &who, Role::TreasuryManager);
        auth::grant_role(&env, &caller, &who, Role::TreasuryManager)?;
        record_treasury_audit(
            &env,
            &caller,
            TimelineEventType::RoleChanged,
            symbol_short!("mgr_grnt"),
            symbol_short!("adm_grant"),
            Some(who.clone()),
            Some(symbol_short!("manager")),
            Some(if was_manager { 1 } else { 0 }),
            Some(1),
        )?;
        emit_permission_changed(
            &env,
            symbol_short!("treasury"),
            symbol_short!("manager"),
            &who,
            true,
            env.ledger().timestamp(),
        );
        Ok(())
    }

    /// Revokes the `TreasuryManager` role from `who`. Admin only.
    pub fn remove_treasury_manager(env: Env, caller: Address, who: Address) -> Result<(), Error> {
        let was_manager = auth::has_role(&env, &who, Role::TreasuryManager);
        auth::revoke_role(&env, &caller, &who, Role::TreasuryManager)?;
        record_treasury_audit(
            &env,
            &caller,
            TimelineEventType::RoleChanged,
            symbol_short!("mgr_rvok"),
            symbol_short!("adm_rvok"),
            Some(who.clone()),
            Some(symbol_short!("manager")),
            Some(if was_manager { 1 } else { 0 }),
            Some(0),
        )?;
        emit_permission_changed(
            &env,
            symbol_short!("treasury"),
            symbol_short!("manager"),
            &who,
            false,
            env.ledger().timestamp(),
        );
        Ok(())
    }

    /// Updates the max per-transaction withdrawal limit. Admin only.
    pub fn set_withdrawal_limit(env: Env, caller: Address, new_limit: i128) -> Result<(), Error> {
        auth::require_admin(&env, &caller)?;
        if new_limit <= 0 {
            return Err(Error::InvalidArgument);
        }
        let previous = instance_get::<_, i128>(&env, &MAX_WD).unwrap_or(0);
        instance_set(&env, &MAX_WD, &new_limit);
        record_treasury_audit(
            &env,
            &caller,
            TimelineEventType::ConfigChanged,
            symbol_short!("wd_limit"),
            symbol_short!("admin_cfg"),
            None,
            None,
            Some(previous),
            Some(new_limit),
        )?;
        emit_action_executed(
            &env,
            symbol_short!("treasury"),
            symbol_short!("wd_limit"),
            &caller,
            true,
            env.ledger().timestamp(),
        );
        Ok(())
    }

    /// Credits `amount` into `category`'s balance for `token`. `TreasuryManager` only.
    pub fn deposit(
        env: Env,
        caller: Address,
        token: Address,
        category: Symbol,
        amount: i128,
    ) -> Result<(), Error> {
        // Cheap validation first — avoids auth commit on trivial rejects.
        if amount <= 0 {
            return Err(Error::InvalidArgument);
        }
        auth::require_permission(&env, &caller, Permission::TreasuryOperations)?;
        let key = (BALANCE, token.clone(), category.clone());
        let balance: i128 = env.storage().instance().get(&key).unwrap_or(0);
        let new_balance = balance.checked_add(amount).ok_or(Error::Overflow)?;
        token::Client::new(&env, &token).transfer(
            &caller,
            &env.current_contract_address(),
            &amount,
        );
        env.storage().instance().set(&key, &new_balance);
        record_treasury_audit(
            &env,
            &caller,
            TimelineEventType::RecordUpdated,
            symbol_short!("deposit"),
            symbol_short!("funds_in"),
            Some(token.clone()),
            Some(category.clone()),
            Some(balance),
            Some(new_balance),
        )?;
        emit_treasury_deposit(&env, category, &caller, &token, amount, new_balance);
        emit_action_executed(
            &env,
            symbol_short!("treasury"),
            symbol_short!("deposit"),
            &caller,
            true,
            env.ledger().timestamp(),
        );
        Ok(())
    }

    /// Returns the current balance for `token` in `category` (0 if never funded).
    pub fn category_balance(env: Env, token: Address, category: Symbol) -> i128 {
        instance_get::<_, i128>(&env, &(BALANCE, token, category)).unwrap_or(0)
    }

    /// Returns the currently configured max per-transaction withdrawal limit.
    pub fn withdrawal_limit(env: Env) -> i128 {
        instance_get::<_, i128>(&env, &MAX_WD).unwrap_or(0)
    }

    /// Withdraws `amount` of `token` from `category` to `to`.
    ///
    /// Guards, in order:
    /// 1. `amount` must be > 0                           → `Error::InvalidArgument`
    /// 2. `amount` must not exceed the withdrawal limit  → `Error::WithdrawalLimitExceeded`
    /// 3. `amount` must not exceed the category balance  → `Error::InsufficientBalance`
    /// 4. `caller` must hold `TreasuryManager`          → `Error::Unauthorized`
    ///
    /// On success, decrements the category balance and emits the shared
    /// `TreasuryWithdrawal` event with `(category, to, token, amount, remaining)`.
    pub fn withdraw(
        env: Env,
        caller: Address,
        token: Address,
        to: Address,
        amount: i128,
        category: Symbol,
    ) -> Result<(), Error> {
        // Gas optimization: cheap validation checks first
        if amount <= 0 {
            return Err(Error::InvalidArgument);
        }

        let limit: i128 = instance_get(&env, &MAX_WD).unwrap_or(0);
        if amount > limit {
            return Err(Error::WithdrawalLimitExceeded);
        }

        let key = (BALANCE, token.clone(), category.clone());
        let balance: i128 = instance_get(&env, &key).unwrap_or(0);
        if amount > balance {
            return Err(Error::InsufficientBalance);
        }

        // Auth check last
        auth::require_permission(&env, &caller, Permission::TreasuryOperations)?;

        // Quota enforcement: fail-open when unconfigured.
        shared::quota::check_and_consume(&env, &caller, &symbol_short!("wdraw"), amount)?;

        let remaining = balance - amount;
        instance_set(&env, &key, &remaining);
        record_treasury_audit(
            &env,
            &caller,
            TimelineEventType::PaymentSent,
            symbol_short!("withdraw"),
            symbol_short!("funds_out"),
            Some(token.clone()),
            Some(category.clone()),
            Some(balance),
            Some(remaining),
        )?;

        emit_treasury_withdrawal(&env, category, &to, &token, amount, remaining);
        emit_action_executed(
            &env,
            symbol_short!("treasury"),
            symbol_short!("withdraw"),
            &caller,
            true,
            env.ledger().timestamp(),
        );

        Ok(())
    }

    /// Emergency reserve withdrawal for `token`.
    ///
    /// Only callable by the admin, and only while the contract is paused.
    /// Intended to move reserve funds to safety when something has gone wrong.
    pub fn emergency_withdraw(
        env: Env,
        caller: Address,
        token: Address,
        to: Address,
        amount: i128,
    ) -> Result<(), Error> {
        // Gas optimization: cheapest validations first.
        if amount <= 0 {
            return Err(Error::InvalidArgument);
        }
        if !shared::storage::is_paused(&env) {
            return Err(Error::NotPaused);
        }

        let key = (BALANCE, token.clone(), RESERVE_CATEGORY);
        let balance: i128 = instance_get(&env, &key).unwrap_or(0);
        let new_balance = balance
            .checked_sub(amount)
            .ok_or(Error::InsufficientBalance)?;
        if new_balance < 0 {
            return Err(Error::InsufficientBalance);
        }

        // Auth check last
        auth::require_admin(&env, &caller)?;
        instance_set(&env, &key, &new_balance);
        record_treasury_audit(
            &env,
            &caller,
            TimelineEventType::PaymentSent,
            symbol_short!("emrg_wd"),
            symbol_short!("emergency"),
            Some(token.clone()),
            Some(RESERVE_CATEGORY),
            Some(balance),
            Some(new_balance),
        )?;

        events::emit(
            &env,
            events::TREASURY_EMERGENCY_WITHDRAW,
            (caller.clone(), token, to, amount),
        );
        emit_action_executed(
            &env,
            symbol_short!("treasury"),
            symbol_short!("emrg_wd"),
            &caller,
            true,
            env.ledger().timestamp(),
        );
        Ok(())
    }

    /// Registers the referral contract address authorised to call
    /// `distribute_reward`. Admin only.
    pub fn set_referral_contract(
        env: Env,
        caller: Address,
        referral_contract: Address,
    ) -> Result<(), Error> {
        auth::require_admin(&env, &caller)?;
        let previous = instance_get::<_, Address>(&env, &REFERRAL_CONTRACT);
        let was_configured = previous.is_some();
        if let Some(previous_contract) = previous {
            auth::revoke_role(&env, &caller, &previous_contract, Role::ServiceActor)?;
        }
        auth::grant_role(&env, &caller, &referral_contract, Role::ServiceActor)?;
        instance_set(&env, &REFERRAL_CONTRACT, &referral_contract);
        record_treasury_audit(
            &env,
            &caller,
            TimelineEventType::ConfigChanged,
            symbol_short!("ref_ctr"),
            symbol_short!("admin_cfg"),
            Some(referral_contract.clone()),
            None,
            Some(if was_configured { 1 } else { 0 }),
            Some(1),
        )?;
        emit_action_executed(
            &env,
            symbol_short!("treasury"),
            symbol_short!("ref_ctr"),
            &caller,
            true,
            env.ledger().timestamp(),
        );
        Ok(())
    }

    /// Returns the currently registered referral contract address, if any.
    pub fn referral_contract(env: Env) -> Option<Address> {
        instance_get(&env, &REFERRAL_CONTRACT)
    }

    /// Pays a referral commission of `amount` in `token` to `recipient` from the
    /// `Rewards` category.
    ///
    /// Callable only by the registered referral contract.
    pub fn distribute_reward(
        env: Env,
        token: Address,
        recipient: Address,
        amount: i128,
    ) -> Result<(), Error> {
        // Cheap validation before auth commit.
        if amount <= 0 {
            return Err(Error::InvalidArgument);
        }

        let key = (BALANCE, token.clone(), REWARDS_CATEGORY);
        let balance: i128 = instance_get(&env, &key).unwrap_or(0);
        if amount > balance {
            return Err(Error::InsufficientBalance);
        }

        // Auth check after cheap validations pass.
        let referral_contract: Address =
            instance_get(&env, &REFERRAL_CONTRACT).ok_or(Error::Unauthorized)?;
        auth::require_permission(&env, &referral_contract, Permission::ServiceOperation)?;

        let remaining = balance - amount;
        instance_set(&env, &key, &remaining);
        record_treasury_audit(
            &env,
            &referral_contract,
            TimelineEventType::PaymentSent,
            symbol_short!("reward"),
            symbol_short!("ref_pay"),
            Some(token.clone()),
            Some(REWARDS_CATEGORY),
            Some(balance),
            Some(remaining),
        )?;

        emit_commission_paid(&env, &recipient, &token, amount, env.ledger().timestamp());
        emit_action_executed(
            &env,
            symbol_short!("treasury"),
            symbol_short!("reward"),
            &referral_contract,
            true,
            env.ledger().timestamp(),
        );

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Quota management (Issue #65) — maintainer diagnostics & overrides
    // -----------------------------------------------------------------------

    /// Set quota limits for a resource (admin only).
    pub fn set_quota_config(
        env: Env,
        caller: Address,
        resource: Symbol,
        config: shared::quota::QuotaConfig,
    ) -> Result<(), Error> {
        auth::require_admin(&env, &caller)?;
        shared::quota::set_quota_config(&env, &resource, &config)?;
        record_treasury_audit(
            &env,
            &caller,
            TimelineEventType::ConfigChanged,
            symbol_short!("quota_cfg"),
            symbol_short!("admin_cfg"),
            None,
            Some(resource.clone()),
            None,
            None,
        )
    }

    /// Inspect quota usage for an actor/resource pair (maintainer diagnostics).
    pub fn quota_status(env: Env, actor: Address, resource: Symbol) -> shared::quota::QuotaStatus {
        shared::quota::get_quota_status(&env, &actor, &resource)
    }

    /// Returns the newest maintainer-only audit entries.
    pub fn audit_trail(
        env: Env,
        maintainer: Address,
        limit: u32,
    ) -> Result<soroban_sdk::Vec<shared::ActionAuditEntry>, Error> {
        shared::timeline::action_audit_trail(&env, &maintainer, limit)
    }

    /// Reset quota usage for an actor/resource pair (admin override path).
    pub fn reset_quota(
        env: Env,
        caller: Address,
        actor: Address,
        resource: Symbol,
    ) -> Result<(), Error> {
        auth::require_admin(&env, &caller)?;
        shared::quota::reset_quota(&env, &actor, &resource);
        record_treasury_audit(
            &env,
            &caller,
            TimelineEventType::ConfigChanged,
            symbol_short!("quota_rst"),
            symbol_short!("adm_ovr"),
            Some(actor.clone()),
            Some(resource.clone()),
            None,
            None,
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod test;

/// CPU/memory regression suite for the treasury's critical entry points.
/// Kept separate from `test` so the behavioural tests and the budget
/// thresholds can be read (and updated) independently.
#[cfg(test)]
mod budget_test;
