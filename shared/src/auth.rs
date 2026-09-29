use soroban_sdk::{contracttype, symbol_short, Address, Env, Symbol};

use crate::errors::Error;
use crate::storage::{persistent_has, persistent_remove, persistent_set};

pub const KEY_ADMIN: Symbol = symbol_short!("admin");

// ---------------------------------------------------------------------------
// Role enum — single authoritative definition
// ---------------------------------------------------------------------------

/// Roles that can be granted to addresses in the system.
///
/// Every role is stored as a persistent `DataKey::Role(address, role)` entry.
/// Roles are independent; holding one does not imply another.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Role {
    /// Full administrative control.
    Admin,
    /// Authorised to trigger contract upgrades.
    Upgrader,
    /// Authorised to move treasury funds.
    TreasuryManager,
    /// Authorised to pause / unpause the contract.
    Pauser,
    /// Authorised to write referral configuration.
    ReferralManager,
    /// Authorised to post oracle signatures / verification proofs.
    OracleSigner,
    /// User-scoped access to resources owned by the address.
    EndUser,
    /// Authorised service or contract actor.
    ServiceActor,
    /// Authorised to perform scoped impersonation for support debugging.
    Support,
}

/// Named capabilities mapped centrally to the role required to exercise them.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Permission {
    /// End-user access remains scoped to records owned by the authenticated address.
    UseOwnResources,
    /// Change contract configuration or manage roles.
    ManageConfiguration,
    /// Grant or revoke roles.
    ManageRoles,
    /// Move treasury funds.
    TreasuryOperations,
    /// Pause or resume a contract.
    PauseContracts,
    /// Change referral configuration.
    ReferralConfiguration,
    /// Submit an oracle update.
    SubmitOracle,
    /// Propose or execute a contract upgrade.
    UpgradeContracts,
    /// Read maintainer-only audit records.
    ReadAuditTrail,
    /// Perform an explicitly registered service operation.
    ServiceOperation,
    /// Perform scoped impersonation for support debugging.
    ImpersonateUser,
}

// ---------------------------------------------------------------------------
// Storage key for role entries — stored in persistent storage
// ---------------------------------------------------------------------------

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataKey {
    /// Presence of this key means `address` holds `role`.
    Role(Address, Role),
}

// ---------------------------------------------------------------------------
// Admin helpers
// ---------------------------------------------------------------------------

/// Stores the admin address during contract initialisation.
pub fn set_admin(env: &Env, admin: &Address) {
    env.storage()
        .instance()
        .set::<Symbol, Address>(&KEY_ADMIN, admin);
}

/// Initializes the shared admin identity exactly once and requires its
/// signature. Also grants the Admin role used by permission checks.
pub fn initialize_admin(env: &Env, admin: &Address) -> Result<(), Error> {
    if env.storage().instance().has(&KEY_ADMIN) {
        return Err(Error::AlreadyInitialized);
    }
    admin.require_auth();
    set_admin(env, admin);
    persistent_set(env, &DataKey::Role(admin.clone(), Role::Admin), &true);
    Ok(())
}

/// Returns the current admin address.
///
/// # Panics
/// Panics if no admin has been set (misconfigured contract).
pub fn get_admin(env: &Env) -> Address {
    env.storage()
        .instance()
        .get::<Symbol, Address>(&KEY_ADMIN)
        .expect("admin not initialised")
}

/// Verifies that `caller` is the admin **and** has provided a valid
/// on-chain signature.
///
/// Returns `Err(Error::Unauthorized)` if either check fails.
pub fn require_admin(env: &Env, caller: &Address) -> Result<(), Error> {
    let admin = get_admin(env);
    if *caller != admin {
        return Err(Error::Unauthorized);
    }
    caller.require_auth();
    Ok(())
}

pub const KEY_PENDING_OWNER: Symbol = symbol_short!("pend_own");

/// Pending ownership transfer record.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingOwnershipTransfer {
    pub current_owner: Address,
    pub target_owner: Address,
    pub expiry: u64,
}

/// Returns the currently pending ownership transfer, if one exists.
pub fn get_pending_ownership_transfer(env: &Env) -> Option<PendingOwnershipTransfer> {
    env.storage().instance().get(&KEY_PENDING_OWNER)
}

/// Proposes an ownership transfer to `target_owner` with an `expiry` timestamp.
/// The caller must be the current admin.
///
/// Returns `Err(Error::Unauthorized)` if the caller is not the admin.
/// Returns `Err(Error::InvalidArgument)` if `expiry <= current_timestamp` or `target_owner == current_owner`.
/// Returns `Err(Error::TransferAlreadyPending)` if an unexpired transfer is already pending.
pub fn propose_ownership_transfer(
    env: &Env,
    current_owner: &Address,
    target_owner: &Address,
    expiry: u64,
) -> Result<(), Error> {
    require_admin(env, current_owner)?;

    let now = env.ledger().timestamp();
    if expiry <= now {
        return Err(Error::InvalidArgument);
    }
    if *target_owner == *current_owner {
        return Err(Error::InvalidArgument);
    }

    if let Some(existing) = get_pending_ownership_transfer(env) {
        if now <= existing.expiry {
            return Err(Error::TransferAlreadyPending);
        }
    }

    let transfer = PendingOwnershipTransfer {
        current_owner: current_owner.clone(),
        target_owner: target_owner.clone(),
        expiry,
    };

    env.storage().instance().set(&KEY_PENDING_OWNER, &transfer);

    let before_hash = env
        .crypto()
        .sha256(&soroban_sdk::Bytes::from_slice(env, &[0]))
        .into();
    let after_hash = env
        .crypto()
        .sha256(&soroban_sdk::Bytes::from_slice(env, &[1]))
        .into();
    crate::history::record_mutation(
        env,
        symbol_short!("admin"),
        current_owner.clone(),
        Symbol::new(env, "prop_own"),
        before_hash,
        after_hash,
    );

    env.events().publish(
        (symbol_short!("auth"), symbol_short!("own_prop")),
        (current_owner.clone(), target_owner.clone(), expiry),
    );

    Ok(())
}

/// Accepts a pending ownership transfer. Caller must be the designated `target_owner`.
///
/// Returns `Err(Error::NoPendingTransfer)` if no transfer is pending.
/// Returns `Err(Error::NotPendingOwner)` if `caller != target_owner`.
/// Returns `Err(Error::TransferExpired)` if the transfer has expired.
pub fn accept_ownership_transfer(env: &Env, caller: &Address) -> Result<(), Error> {
    caller.require_auth();

    let transfer = get_pending_ownership_transfer(env).ok_or(Error::NoPendingTransfer)?;

    if *caller != transfer.target_owner {
        return Err(Error::NotPendingOwner);
    }

    let now = env.ledger().timestamp();
    if now > transfer.expiry {
        env.storage().instance().remove(&KEY_PENDING_OWNER);
        return Err(Error::TransferExpired);
    }

    env.storage().instance().remove(&KEY_PENDING_OWNER);

    // Update stored admin address
    set_admin(env, caller);

    // Grant Admin role to new owner
    persistent_set(env, &DataKey::Role(caller.clone(), Role::Admin), &true);

    // Revoke Admin role from old owner
    persistent_remove(
        env,
        &DataKey::Role(transfer.current_owner.clone(), Role::Admin),
    );

    let before_hash = env
        .crypto()
        .sha256(&soroban_sdk::Bytes::from_slice(env, &[0]))
        .into();
    let after_hash = env
        .crypto()
        .sha256(&soroban_sdk::Bytes::from_slice(env, &[1]))
        .into();
    crate::history::record_mutation(
        env,
        symbol_short!("admin"),
        caller.clone(),
        Symbol::new(env, "acpt_own"),
        before_hash,
        after_hash,
    );

    env.events().publish(
        (symbol_short!("auth"), symbol_short!("own_acpt")),
        (transfer.current_owner, caller.clone()),
    );

    Ok(())
}

/// Cancels an active pending ownership transfer. Caller must be the current admin.
///
/// Returns `Err(Error::Unauthorized)` if caller is not the admin.
/// Returns `Err(Error::NoPendingTransfer)` if no transfer is pending.
pub fn cancel_ownership_transfer(env: &Env, caller: &Address) -> Result<(), Error> {
    require_admin(env, caller)?;

    let transfer = get_pending_ownership_transfer(env).ok_or(Error::NoPendingTransfer)?;

    env.storage().instance().remove(&KEY_PENDING_OWNER);

    let before_hash = env
        .crypto()
        .sha256(&soroban_sdk::Bytes::from_slice(env, &[1]))
        .into();
    let after_hash = env
        .crypto()
        .sha256(&soroban_sdk::Bytes::from_slice(env, &[0]))
        .into();
    crate::history::record_mutation(
        env,
        symbol_short!("admin"),
        caller.clone(),
        Symbol::new(env, "canc_own"),
        before_hash,
        after_hash,
    );

    env.events().publish(
        (symbol_short!("auth"), symbol_short!("own_canc")),
        (caller.clone(), transfer.target_owner),
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Role storage helpers
// ---------------------------------------------------------------------------

/// Returns `true` when `user` holds `role`.
///
/// Uses [`persistent_get`] so the role entry's TTL is extended on every
/// successful check, keeping frequently-queried roles alive.
pub fn has_role(env: &Env, user: &Address, role: Role) -> bool {
    persistent_has(env, &DataKey::Role(user.clone(), role))
}

/// Grants `role` to `user`. Admin-gated — `admin_caller` must be the
/// current admin with a valid signature.
///
/// Returns `Err(Error::Unauthorized)` when the caller is not the admin.
/// The role entry is written to persistent storage with an immediate TTL
/// extension so it survives upcoming ledger closures.
pub fn grant_role(
    env: &Env,
    admin_caller: &Address,
    user: &Address,
    role: Role,
) -> Result<(), Error> {
    require_admin(env, admin_caller)?;
    let key = DataKey::Role(user.clone(), role.clone());

    // Hash based on state (false -> true)
    let before_hash = env
        .crypto()
        .sha256(&soroban_sdk::Bytes::from_slice(env, &[0]))
        .into();
    let after_hash = env
        .crypto()
        .sha256(&soroban_sdk::Bytes::from_slice(env, &[1]))
        .into();

    crate::history::record_mutation(
        env,
        soroban_sdk::symbol_short!("role"),
        admin_caller.clone(),
        soroban_sdk::Symbol::new(env, "grant"),
        before_hash,
        after_hash,
    );

    persistent_set(env, &key, &true);
    Ok(())
}

/// Revokes `role` from `user`. Admin-gated — `admin_caller` must be the
/// current admin with a valid signature.
///
/// Returns `Err(Error::Unauthorized)` when the caller is not the admin.
///
/// # Idempotency
/// If `user` does not currently hold `role` this is a no-op and returns
/// `Ok(())`.
pub fn revoke_role(
    env: &Env,
    admin_caller: &Address,
    user: &Address,
    role: Role,
) -> Result<(), Error> {
    require_admin(env, admin_caller)?;
    let key = DataKey::Role(user.clone(), role.clone());
    if persistent_has(env, &key) {
        let before_hash = env
            .crypto()
            .sha256(&soroban_sdk::Bytes::from_slice(env, &[1]))
            .into();
        let after_hash = env
            .crypto()
            .sha256(&soroban_sdk::Bytes::from_slice(env, &[0]))
            .into();

        crate::history::record_mutation(
            env,
            soroban_sdk::symbol_short!("role"),
            admin_caller.clone(),
            soroban_sdk::Symbol::new(env, "revoke"),
            before_hash,
            after_hash,
        );
        persistent_remove(env, &key);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Role guard
// ---------------------------------------------------------------------------

/// Verifies that `caller` holds `role` **and** has provided a valid
/// on-chain signature.
///
/// Returns `Err(Error::Unauthorized)` if either check fails.
pub fn require_role(env: &Env, caller: &Address, role: Role) -> Result<(), Error> {
    if !has_role(env, caller, role) {
        return Err(Error::Unauthorized);
    }
    caller.require_auth();
    Ok(())
}

/// Returns the role required for a named capability.
pub fn role_for_permission(permission: Permission) -> Role {
    match permission {
        Permission::UseOwnResources => Role::EndUser,
        Permission::ManageConfiguration | Permission::ManageRoles | Permission::ReadAuditTrail => {
            Role::Admin
        }
        Permission::TreasuryOperations => Role::TreasuryManager,
        Permission::PauseContracts => Role::Pauser,
        Permission::ReferralConfiguration => Role::ReferralManager,
        Permission::SubmitOracle => Role::OracleSigner,
        Permission::UpgradeContracts => Role::Upgrader,
        Permission::ServiceOperation => Role::ServiceActor,
        Permission::ImpersonateUser => Role::Support,
    }
}

/// Returns whether `user` holds the role required for `permission`.
pub fn has_permission(env: &Env, user: &Address, permission: Permission) -> bool {
    if has_role(env, user, role_for_permission(permission.clone())) {
        return true;
    }

    match permission {
        Permission::UseOwnResources | Permission::ServiceOperation | Permission::SubmitOracle => {
            false
        }
        _ => has_admin_authority(env, user),
    }
}

/// Checks a named capability and the caller's on-chain signature.
pub fn require_permission(
    env: &Env,
    caller: &Address,
    permission: Permission,
) -> Result<(), Error> {
    if !has_permission(env, caller, permission) {
        return Err(Error::Unauthorized);
    }
    caller.require_auth();
    Ok(())
}

fn has_admin_authority(env: &Env, user: &Address) -> bool {
    if has_role(env, user, Role::Admin) {
        return true;
    }

    env.storage()
        .instance()
        .get::<Symbol, Address>(&KEY_ADMIN)
        .map(|admin| admin == *user)
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Pause guard
// ---------------------------------------------------------------------------

/// Returns `Err(Error::ContractPaused)` when the contract is paused,
/// and `Ok(())` when it is active.
///
/// Call this at the top of every state-changing entry point.
pub fn require_not_paused(env: &Env) -> Result<(), Error> {
    if crate::storage::is_paused(env) {
        return Err(Error::ContractPaused);
    }
    Ok(())
}
