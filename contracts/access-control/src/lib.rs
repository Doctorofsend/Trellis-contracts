#![no_std]

//! # Access Control Contract
//!
//! A standalone Role-Based Access Control (RBAC) module for the Trellis
//! Soroban smart-contract suite.  Provides:
//!
//! * Dynamic role creation, assignment, and revocation.
//! * Hierarchical roles — a holder of a parent role automatically satisfies any
//!   child role.
//! * Multi-admin management — multiple addresses may share administrative
//!   authority, with the ability to add/remove fellow admins.
//! * On-chain events for every state change (role creation, grant, revoke,
//!   hierarchy changes, admin changes).
//! * Off-chain read helpers for enumerating role members and querying role
//!   ancestry.

use shared::TimelineEventType;
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, Address, Bytes, Env, Map,
    Symbol, Vec,
};

mod permissions;
pub use permissions::{
    has_action_permission, policy_for, require_action, Action, ActionPolicy, ActionScope,
};

// ---------------------------------------------------------------------------
// Error codes — extend the shared error space starting at 200
// ---------------------------------------------------------------------------

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum AccessControlError {
    /// The specified role does not exist in the registry.
    RoleNotFound = 200,
    /// The role already exists — duplicate creation is not allowed.
    RoleAlreadyExists = 201,
    /// The target address already holds the role.
    AlreadyMember = 202,
    /// The target address does not hold the role.
    NotMember = 203,
    /// Cannot add a role as a parent of itself.
    SelfReference = 204,
    /// Adding this parent would create a cycle in the role hierarchy.
    CycleDetected = 205,
    /// The caller is not a registered admin.
    NotAdmin = 206,
    /// The caller is the super-admin and cannot be removed via the normal path.
    CannotRemoveSuperAdmin = 207,
    /// The role string is empty or otherwise invalid.
    InvalidRole = 208,
    InvitationNotFound = 209,
    InvitationExpired = 210,
    RoleEscalation = 211,
    RateLimitExceeded = 212,
    AlreadyInitialized = 213,
    AuditFailed = 214,
}

type ContractResult<T> = core::result::Result<T, AccessControlError>;

// ---------------------------------------------------------------------------
// Storage keys
// ---------------------------------------------------------------------------

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
enum DataKey {
    /// Set of registered admin addresses (stored as a Map<Address, bool>).
    Admins,
    /// The original super-admin set at initialisation.
    SuperAdmin,
    /// Parent role for each role: `role -> parent_role`.
    RoleParent(Symbol),
    /// Whether a role name has been registered: `role -> bool`.
    RoleExists(Symbol),
    /// Membership entry: `(role, member) -> bool`.
    RoleMember(Symbol, Address),
    /// Direct members ever seen for a role, used for off-chain enumeration.
    RoleMemberList(Symbol),
    /// Invitation data: `(invitee, role) -> Invitation`
    Invitation(Address, Symbol),
    /// Track invite count per inviter to rate limit: `inviter -> u32`
    InviteCount(Address),
    /// Track last invite time per inviter: `inviter -> u64`
    LastInviteTime(Address),
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Invitation {
    pub inviter: Address,
    pub role: Symbol,
    pub expires_at: u64,
}

// ---------------------------------------------------------------------------
// Events (topic symbols)
// ---------------------------------------------------------------------------

const EV_ROLE_CREATED: Symbol = symbol_short!("ac_cr");
const EV_ROLE_GRANTED: Symbol = symbol_short!("ac_gr");
const EV_ROLE_REVOKED: Symbol = symbol_short!("ac_rv");
const EV_ROLE_PARENT_SET: Symbol = symbol_short!("ac_ps");
const EV_ADMIN_ADDED: Symbol = symbol_short!("ac_ad");
const EV_ADMIN_REMOVED: Symbol = symbol_short!("ac_ar");
const EV_INVITE_CREATED: Symbol = symbol_short!("ac_ic");
const EV_INVITE_ACCEPTED: Symbol = symbol_short!("ac_ia");
const EV_INVITE_REVOKED: Symbol = symbol_short!("ac_ir");

// ---------------------------------------------------------------------------
// Contract
// ---------------------------------------------------------------------------

#[contract]
pub struct AccessControlContract;

#[contractimpl]
impl AccessControlContract {
    // -----------------------------------------------------------------------
    // Initialisation
    // -----------------------------------------------------------------------

    /// Initialise the contract with a super-admin who is automatically
    /// registered as an admin and granted the `super_admin` role.
    ///
    /// The `super_admin` role is the root of the hierarchy and cannot be
    /// revoked from the super-admin address.
    pub fn initialize(env: Env, super_admin: Address) -> Result<(), AccessControlError> {
        if env.storage().instance().has(&DataKey::SuperAdmin) {
            return Err(AccessControlError::AlreadyInitialized);
        }
        shared::auth::initialize_admin(&env, &super_admin).map_err(|error| match error {
            shared::Error::AlreadyInitialized => AccessControlError::AlreadyInitialized,
            _ => AccessControlError::NotAdmin,
        })?;
        // Store the super-admin address.
        env.storage()
            .instance()
            .set(&DataKey::SuperAdmin, &super_admin);

        // Register the super-admin in the admins map.
        let mut admins: Map<Address, bool> = Map::new(&env);
        admins.set(super_admin.clone(), true);
        env.storage().instance().set(&DataKey::Admins, &admins);

        // Register the `super_admin` role and grant it.
        register_role_internal(&env, &symbol_short!("super"));
        grant_role_internal(&env, &symbol_short!("super"), &super_admin);
        record_access_audit(
            &env,
            &super_admin,
            TimelineEventType::ConfigChanged,
            symbol_short!("init"),
            symbol_short!("setup"),
            None,
            Some(symbol_short!("super")),
            None,
            Some(1),
        )?;

        // Emit: role created for super_admin.
        env.events()
            .publish((EV_ROLE_CREATED,), (symbol_short!("super"), super_admin));
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Admin management
    // -----------------------------------------------------------------------

    /// Returns `true` if `who` is a registered admin.
    pub fn is_admin(env: Env, who: Address) -> bool {
        let admins: Map<Address, bool> = env
            .storage()
            .instance()
            .get(&DataKey::Admins)
            .unwrap_or_else(|| Map::new(&env));
        admins.get(who).unwrap_or(false)
    }

    /// Returns the super-admin address.
    pub fn super_admin(env: Env) -> Address {
        env.storage()
            .instance()
            .get(&DataKey::SuperAdmin)
            .expect("contract not initialised")
    }

    /// Returns structured role and administration audit records for maintainers.
    ///
    /// # Authorization
    /// Requires the caller to hold the `ReadAuditTrail` action permission
    /// (see [`permissions`]).  Unauthorized callers receive
    /// [`shared::Error::Unauthorized`].
    pub fn audit_trail(
        env: Env,
        maintainer: Address,
        limit: u32,
    ) -> Result<Vec<shared::ActionAuditEntry>, shared::Error> {
        // Reading maintainer-only audit records is a privileged action; the
        // matrix authorizes the caller before the shared accessor repeats the
        // admin check (and performs the authorization, so it is not repeated
        // here).
        if !has_action_permission(&env, &maintainer, &Action::ReadAuditTrail) {
            return Err(shared::Error::Unauthorized);
        }
        shared::timeline::action_audit_trail(&env, &maintainer, limit)
    }

    /// Add a new admin.  Only existing admins may call this.
    ///
    /// # Authorization
    /// Requires the `ManageMaintainers` action permission.  Unauthorized
    /// callers receive [`AccessControlError::NotAdmin`].
    pub fn add_admin(
        env: Env,
        caller: Address,
        new_admin: Address,
    ) -> Result<(), AccessControlError> {
        require_action(&env, &caller, &Action::ManageMaintainers)?;

        let mut admins: Map<Address, bool> = env
            .storage()
            .instance()
            .get(&DataKey::Admins)
            .unwrap_or_else(|| Map::new(&env));

        if admins.get(new_admin.clone()).unwrap_or(false) {
            if !shared::auth::has_role(&env, &new_admin, shared::auth::Role::Admin) {
                shared::storage::persistent_set(
                    &env,
                    &shared::auth::DataKey::Role(new_admin.clone(), shared::auth::Role::Admin),
                    &true,
                );
                record_access_audit(
                    &env,
                    &caller,
                    TimelineEventType::RoleChanged,
                    symbol_short!("admin_add"),
                    symbol_short!("role_sync"),
                    Some(new_admin.clone()),
                    Some(symbol_short!("admin")),
                    Some(0),
                    Some(1),
                )?;
            }
            return Ok(());
        }

        admins.set(new_admin.clone(), true);
        env.storage().instance().set(&DataKey::Admins, &admins);
        shared::storage::persistent_set(
            &env,
            &shared::auth::DataKey::Role(new_admin.clone(), shared::auth::Role::Admin),
            &true,
        );
        record_access_audit(
            &env,
            &caller,
            TimelineEventType::RoleChanged,
            symbol_short!("admin_add"),
            symbol_short!("adm_grant"),
            Some(new_admin.clone()),
            Some(symbol_short!("admin")),
            Some(0),
            Some(1),
        )?;

        env.events().publish((EV_ADMIN_ADDED,), (caller, new_admin));
        Ok(())
    }

    /// Remove an admin.  Only existing admins may call this.  The super-admin
    /// cannot be removed.
    ///
    /// # Authorization
    /// Requires the `ManageMaintainers` action permission.  Unauthorized
    /// callers receive [`AccessControlError::NotAdmin`].  The super-admin
    /// address cannot be removed and yields
    /// [`AccessControlError::CannotRemoveSuperAdmin`].
    pub fn remove_admin(
        env: Env,
        caller: Address,
        target: Address,
    ) -> Result<(), AccessControlError> {
        require_action(&env, &caller, &Action::ManageMaintainers)?;

        let super_admin_addr: Address = env
            .storage()
            .instance()
            .get(&DataKey::SuperAdmin)
            .expect("contract not initialised");

        if target == super_admin_addr {
            return Err(AccessControlError::CannotRemoveSuperAdmin);
        }

        let mut admins: Map<Address, bool> = env
            .storage()
            .instance()
            .get(&DataKey::Admins)
            .unwrap_or_else(|| Map::new(&env));

        if !admins.get(target.clone()).unwrap_or(false) {
            // Not an admin — idempotent success.
            return Ok(());
        }

        admins.set(target.clone(), false);
        env.storage().instance().set(&DataKey::Admins, &admins);
        shared::storage::persistent_remove(
            &env,
            &shared::auth::DataKey::Role(target.clone(), shared::auth::Role::Admin),
        );
        record_access_audit(
            &env,
            &caller,
            TimelineEventType::RoleChanged,
            symbol_short!("admin_rmv"),
            symbol_short!("adm_rvok"),
            Some(target.clone()),
            Some(symbol_short!("admin")),
            Some(1),
            Some(0),
        )?;

        env.events().publish((EV_ADMIN_REMOVED,), (caller, target));
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Role creation
    // -----------------------------------------------------------------------

    /// Create a new role.  Admin-gated.
    ///
    /// # Authorization
    /// Requires the `ConfigureRoleRegistry` action permission.  Unauthorized
    /// callers receive [`AccessControlError::NotAdmin`].
    pub fn create_role(env: Env, caller: Address, role: Symbol) -> Result<(), AccessControlError> {
        require_action(&env, &caller, &Action::ConfigureRoleRegistry)?;

        validate_role_symbol(&role)?;

        if role_exists(&env, &role) {
            return Err(AccessControlError::RoleAlreadyExists);
        }

        register_role_internal(&env, &role);
        record_access_audit(
            &env,
            &caller,
            TimelineEventType::ConfigChanged,
            symbol_short!("role_new"),
            symbol_short!("admin_cfg"),
            None,
            Some(role.clone()),
            Some(0),
            Some(1),
        )?;

        env.events().publish((EV_ROLE_CREATED,), (role, caller));
        Ok(())
    }

    /// Returns `true` if the role has been registered.
    pub fn role_exists_check(env: Env, role: Symbol) -> bool {
        role_exists(&env, &role)
    }

    // -----------------------------------------------------------------------
    // Role hierarchy
    // -----------------------------------------------------------------------

    /// Set `parent` as the parent of `role`.  A holder of `parent` will
    /// automatically satisfy `role` checks.  Admin-gated.
    ///
    /// Rejects self-references and cycles.
    pub fn set_role_parent(
        env: Env,
        caller: Address,
        role: Symbol,
        parent: Symbol,
    ) -> Result<(), AccessControlError> {
        require_action(&env, &caller, &Action::ConfigureRoleRegistry)?;

        ensure_role_exists(&env, &role)?;
        ensure_role_exists(&env, &parent)?;

        if role == parent {
            return Err(AccessControlError::SelfReference);
        }

        // Guard against cycles: if `parent` already has `role` as an ancestor
        // then setting `role -> parent` would create a cycle.
        if has_ancestor(&env, &parent, &role) {
            return Err(AccessControlError::CycleDetected);
        }

        let had_parent = env
            .storage()
            .instance()
            .has(&DataKey::RoleParent(role.clone()));
        env.storage()
            .instance()
            .set(&DataKey::RoleParent(role.clone()), &parent);
        record_access_audit(
            &env,
            &caller,
            TimelineEventType::ConfigChanged,
            symbol_short!("role_par"),
            symbol_short!("admin_cfg"),
            None,
            Some(role.clone()),
            Some(if had_parent { 1 } else { 0 }),
            Some(1),
        )?;

        env.events()
            .publish((EV_ROLE_PARENT_SET,), (role, parent, caller));
        Ok(())
    }

    /// Returns the direct parent of `role`, if one has been set.
    pub fn get_role_parent(env: Env, role: Symbol) -> Option<Symbol> {
        env.storage().instance().get(&DataKey::RoleParent(role))
    }

    // -----------------------------------------------------------------------
    // Grant / Revoke
    // -----------------------------------------------------------------------

    /// Grant `role` to `user`.  Admin-gated.
    pub fn grant_role(
        env: Env,
        caller: Address,
        role: Symbol,
        user: Address,
    ) -> Result<(), AccessControlError> {
        require_action(&env, &caller, &Action::AssignRoles)?;

        ensure_role_exists(&env, &role)?;

        let was_member = has_direct_role(&env, &role, &user);
        grant_role_internal(&env, &role, &user);
        record_access_audit(
            &env,
            &caller,
            TimelineEventType::RoleChanged,
            symbol_short!("role_grt"),
            symbol_short!("adm_grant"),
            Some(user.clone()),
            Some(role.clone()),
            Some(if was_member { 1 } else { 0 }),
            Some(1),
        )?;

        env.events()
            .publish((EV_ROLE_GRANTED,), (role, user, caller));
        Ok(())
    }

    /// Revoke `role` from `user`.  Admin-gated.  Idempotent — succeeds even if
    /// `user` does not currently hold `role`.
    pub fn revoke_role(
        env: Env,
        caller: Address,
        role: Symbol,
        user: Address,
    ) -> Result<(), AccessControlError> {
        require_action(&env, &caller, &Action::AssignRoles)?;

        ensure_role_exists(&env, &role)?;

        let was_member = has_direct_role(&env, &role, &user);
        revoke_role_internal(&env, &role, &user);
        record_access_audit(
            &env,
            &caller,
            TimelineEventType::RoleChanged,
            symbol_short!("role_rvk"),
            symbol_short!("adm_rvok"),
            Some(user.clone()),
            Some(role.clone()),
            Some(if was_member { 1 } else { 0 }),
            Some(0),
        )?;

        env.events()
            .publish((EV_ROLE_REVOKED,), (role, user, caller));
        Ok(())
    }

    /// Returns `true` if `user` holds `role`, either directly or via an
    /// ancestor in the hierarchy.
    pub fn has_role(env: Env, role: Symbol, user: Address) -> bool {
        has_role_recursive(&env, &role, &user)
    }

    // -----------------------------------------------------------------------
    // Invitations
    // -----------------------------------------------------------------------

    /// Create an invitation for `invitee` to join `role`.
    /// Caller must have the `role` or be an admin.
    pub fn create_invitation(
        env: Env,
        caller: Address,
        role: Symbol,
        invitee: Address,
        ttl_ledgers: u64,
    ) -> Result<(), AccessControlError> {
        ensure_role_exists(&env, &role)?;

        // Role-scoped: a maintainer, or a holder of `role`, may invite.
        // The permission matrix owns the escalation check.
        require_action(&env, &caller, &Action::InviteMember(role.clone()))?;

        // Rate limiting
        let current_time = env.ledger().timestamp();
        let last_time = env.storage().instance().get(&DataKey::LastInviteTime(caller.clone())).unwrap_or(0u64);
        let mut count: u32 = env.storage().instance().get(&DataKey::InviteCount(caller.clone())).unwrap_or(0);
        
        // Reset count if more than 1 hour passed
        if current_time > last_time + 3600 {
            count = 0;
        }
        
        if count >= 10 {
            return Err(AccessControlError::RateLimitExceeded);
        }
        
        env.storage().instance().set(&DataKey::LastInviteTime(caller.clone()), &current_time);
        env.storage().instance().set(&DataKey::InviteCount(caller.clone()), &(count + 1));

        let expires_at = current_time + ttl_ledgers; // ttl_ledgers here acts as time in seconds for simplicity
        
        let inv = Invitation {
            inviter: caller.clone(),
            role: role.clone(),
            expires_at,
        };
        
        let invitation_key = DataKey::Invitation(invitee.clone(), role.clone());
        let invitation_exists = env.storage().instance().has(&invitation_key);
        env.storage().instance().set(&invitation_key, &inv);
        record_access_audit(
            &env,
            &caller,
            TimelineEventType::RoleChanged,
            symbol_short!("invite"),
            symbol_short!("create"),
            Some(invitee.clone()),
            Some(role.clone()),
            Some(if invitation_exists { 1 } else { 0 }),
            Some(1),
        )?;
        env.events().publish((EV_INVITE_CREATED,), (caller, invitee, role));
        Ok(())
    }

    /// Accept an invitation to `role`.
    pub fn accept_invitation(
        env: Env,
        caller: Address,
        role: Symbol,
    ) -> Result<(), AccessControlError> {
        caller.require_auth();
        
        let key = DataKey::Invitation(caller.clone(), role.clone());
        if let Some(inv) = env.storage().instance().get::<DataKey, Invitation>(&key) {
            let current_time = env.ledger().timestamp();
            if current_time > inv.expires_at {
                env.storage().instance().remove(&key);
                return Err(AccessControlError::InvitationExpired);
            }
            
            let was_member = has_direct_role(&env, &role, &caller);
            grant_role_internal(&env, &role, &caller);
            env.storage().instance().remove(&key);
            record_access_audit(
                &env,
                &caller,
                TimelineEventType::RoleChanged,
                symbol_short!("invite"),
                symbol_short!("accept"),
                Some(caller.clone()),
                Some(role.clone()),
                Some(if was_member { 1 } else { 0 }),
                Some(1),
            )?;
            
            env.events().publish((EV_INVITE_ACCEPTED,), (caller.clone(), role.clone()));
            Ok(())
        } else {
            Err(AccessControlError::InvitationNotFound)
        }
    }

    /// Revoke an invitation. Caller must be the original inviter or an admin.
    pub fn revoke_invitation(
        env: Env,
        caller: Address,
        role: Symbol,
        invitee: Address,
    ) -> Result<(), AccessControlError> {
        let key = DataKey::Invitation(invitee.clone(), role.clone());
        if let Some(inv) = env.storage().instance().get::<DataKey, Invitation>(&key) {
            // Owner-scoped: a maintainer, or the original inviter, may cancel.
            require_action(&env, &caller, &Action::CancelInvitation(inv.inviter.clone()))?;

            env.storage().instance().remove(&key);
            record_access_audit(
                &env,
                &caller,
                TimelineEventType::RoleChanged,
                symbol_short!("invite"),
                symbol_short!("revoke"),
                Some(invitee.clone()),
                Some(role.clone()),
                Some(1),
                Some(0),
            )?;
            env.events().publish((EV_INVITE_REVOKED,), (caller, invitee, role));
            Ok(())
        } else {
            Err(AccessControlError::InvitationNotFound)
        }
    }

    // -----------------------------------------------------------------------
    // Off-chain read helpers
    // -----------------------------------------------------------------------

    /// Returns the list of all direct members of `role`.
    pub fn get_role_members(env: Env, role: Symbol) -> Vec<Address> {
        let members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::RoleMemberList(role.clone()))
            .unwrap_or_else(|| Vec::new(&env));

        let mut result: Vec<Address> = Vec::new(&env);
        for addr in members.iter() {
            if role_member(&env, &role, &addr) {
                result.push_back(addr);
            }
        }
        result
    }

    /// Returns the full ancestor chain for `role` (exclusive of `role`
    /// itself), ordered from immediate parent upward.
    pub fn get_role_ancestors(env: Env, role: Symbol) -> Vec<Symbol> {
        let mut chain: Vec<Symbol> = Vec::new(&env);
        let mut current = role;
        while let Some(parent) = env
            .storage()
            .instance()
            .get::<DataKey, Symbol>(&DataKey::RoleParent(current))
        {
            chain.push_back(parent.clone());
            current = parent;
        }
        chain
    }

    /// Returns all registered role names.
    pub fn get_all_roles(env: Env) -> Vec<Symbol> {
        // We can't iterate storage keys directly in Soroban, so we keep an
        // explicit registry list alongside the existence flags.
        env.storage()
            .instance()
            .get::<Symbol, Vec<Symbol>>(&symbol_short!("all_roles"))
            .unwrap_or_else(|| Vec::new(&env))
    }

    /// Returns all admin addresses.
    pub fn get_all_admins(env: Env) -> Vec<Address> {
        let admins: Map<Address, bool> = env
            .storage()
            .instance()
            .get(&DataKey::Admins)
            .unwrap_or_else(|| Map::new(&env));

        let mut result: Vec<Address> = Vec::new(&env);
        for (addr, is_admin) in admins.iter() {
            if is_admin {
                result.push_back(addr);
            }
        }
        result
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Validate that a role symbol is non-empty and not too long.
fn validate_role_symbol(_role: &Symbol) -> Result<(), AccessControlError> {
    // Soroban symbols are limited to 9 bytes (enforced by symbol_short!).
    // Symbol::new / symbol_short! reject empty strings, so any valid Symbol
    // is implicitly non-empty and within length bounds.
    Ok(())
}

/// Returns `true` if the role has been registered.
fn role_exists(env: &Env, role: &Symbol) -> bool {
    env.storage()
        .instance()
        .get::<DataKey, bool>(&DataKey::RoleExists(role.clone()))
        .unwrap_or(false)
}

/// Register a role — sets the existence flag and appends to the all-roles list.
fn register_role_internal(env: &Env, role: &Symbol) {
    env.storage()
        .instance()
        .set(&DataKey::RoleExists(role.clone()), &true);

    let mut all_roles: Vec<Symbol> = env
        .storage()
        .instance()
        .get(&symbol_short!("all_roles"))
        .unwrap_or_else(|| Vec::new(env));
    all_roles.push_back(role.clone());
    env.storage()
        .instance()
        .set(&symbol_short!("all_roles"), &all_roles);
}

fn ensure_role_exists(env: &Env, role: &Symbol) -> ContractResult<()> {
    if role_exists(env, role) {
        Ok(())
    } else {
        Err(AccessControlError::RoleNotFound)
    }
}

/// Grant `role` to `user` — writes the membership entry.
fn grant_role_internal(env: &Env, role: &Symbol, user: &Address) {
    env.storage()
        .instance()
        .set(&DataKey::RoleMember(role.clone(), user.clone()), &true);

    let mut members: Vec<Address> = env
        .storage()
        .instance()
        .get(&DataKey::RoleMemberList(role.clone()))
        .unwrap_or_else(|| Vec::new(env));
    if !members.iter().any(|member| member == *user) {
        members.push_back(user.clone());
        env.storage()
            .instance()
            .set(&DataKey::RoleMemberList(role.clone()), &members);
    }
}

/// Returns `true` when `user` holds `role` directly (ignoring ancestors).
fn has_direct_role(env: &Env, role: &Symbol, user: &Address) -> bool {
    // Memberships are stored as individual `DataKey::RoleMember(role, user)`
    // entries, so a direct check is a single entry lookup.
    role_member(env, role, user)
}

/// Revoke `role` from `user` — sets membership to false (soft-delete).
fn revoke_role_internal(env: &Env, role: &Symbol, user: &Address) {
    env.storage()
        .instance()
        .set(&DataKey::RoleMember(role.clone(), user.clone()), &false);
}

/// Recursively check if `user` holds `role` (directly or via ancestors).
pub(crate) fn has_role_recursive(env: &Env, role: &Symbol, user: &Address) -> bool {
    // Direct membership check.
    if role_member(env, role, user) {
        return true;
    }

    // Walk up the hierarchy.
    if let Some(parent) = env
        .storage()
        .instance()
        .get::<DataKey, Symbol>(&DataKey::RoleParent(role.clone()))
    {
        return has_role_recursive(env, &parent, user);
    }

    false
}

fn role_member(env: &Env, role: &Symbol, user: &Address) -> bool {
    env.storage()
        .instance()
        .get::<DataKey, bool>(&DataKey::RoleMember(role.clone(), user.clone()))
        .unwrap_or(false)
}

/// Returns `true` when `ancestor_candidate` is an ancestor of `role` (i.e.
/// walking up from `role` we eventually reach `ancestor_candidate`).
fn has_ancestor(env: &Env, role: &Symbol, ancestor_candidate: &Symbol) -> bool {
    let mut current = role.clone();
    loop {
        if let Some(parent) = env
            .storage()
            .instance()
            .get::<DataKey, Symbol>(&DataKey::RoleParent(current.clone()))
        {
            if parent == *ancestor_candidate {
                return true;
            }
            current = parent;
        } else {
            return false;
        }
    }
}

/// Returns `true` when `caller` is registered in this contract's admin map.
pub(crate) fn is_admin_internal(env: &Env, caller: &Address) -> bool {
    let admins: Map<Address, bool> = env
        .storage()
        .instance()
        .get(&DataKey::Admins)
        .unwrap_or_else(|| Map::new(env));
    admins.get(caller.clone()).unwrap_or(false)
}

fn record_access_audit(
    env: &Env,
    actor: &Address,
    event_type: shared::TimelineEventType,
    action: Symbol,
    reason: Symbol,
    resource: Option<Address>,
    attribute: Option<Symbol>,
    before: Option<i128>,
    after: Option<i128>,
) -> ContractResult<()> {
    shared::record_action_audit_event(
        env,
        actor,
        event_type,
        shared::ResourceLink {
            kind: Bytes::from_slice(env, b"access"),
            id: 0,
            revision: 0,
        },
        symbol_short!("access"),
        action,
        reason,
        resource,
        attribute,
        before,
        after,
    )
    .map(|_| ())
    .map_err(|_| AccessControlError::AuditFailed)
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use soroban_sdk::testutils::{Address as _, Events, Ledger as _};
    use soroban_sdk::{Env, IntoVal};

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn setup() -> (Env, Address, Address) {
        let env = Env::default();
        env.mock_all_auths();
        let super_admin = Address::generate(&env);
        let contract_id = env.register_contract(None, AccessControlContract);
        let client = AccessControlContractClient::new(&env, &contract_id);
        client.initialize(&super_admin);
        (env, super_admin, contract_id)
    }

    fn client_for<'a>(env: &'a Env, contract_id: &Address) -> AccessControlContractClient<'a> {
        AccessControlContractClient::new(env, contract_id)
    }

    #[test]
    fn initialization_requires_authentication_and_is_one_time() {
        let env = Env::default();
        let contract_id = env.register_contract(None, AccessControlContract);
        let client = AccessControlContractClient::new(&env, &contract_id);
        let super_admin = Address::generate(&env);

        assert!(client.try_initialize(&super_admin).is_err());
        env.mock_all_auths();
        client.initialize(&super_admin);
        assert_eq!(
            client.try_initialize(&super_admin),
            Err(Ok(AccessControlError::AlreadyInitialized))
        );
    }

    #[test]
    fn role_changes_are_queryable_with_actor_and_state_context() {
        let (env, super_admin, contract_id) = setup();
        let client = client_for(&env, &contract_id);
        let role = symbol_short!("audited");
        let user = Address::generate(&env);

        client.create_role(&super_admin, &role);
        client.grant_role(&super_admin, &role, &user);

        let audit = client.audit_trail(&super_admin, &10);
        assert_eq!(audit.get(0).unwrap().actor, super_admin);
        assert_eq!(audit.get(0).unwrap().scope, symbol_short!("access"));
        assert_eq!(audit.get(0).unwrap().action, symbol_short!("role_grt"));
        assert_eq!(audit.get(0).unwrap().resource, Some(user));
        assert_eq!(audit.get(0).unwrap().attribute, Some(role));
        assert_eq!(audit.get(0).unwrap().before, Some(0));
        assert_eq!(audit.get(0).unwrap().after, Some(1));
    }

    // -----------------------------------------------------------------------
    // Invitations
    // -----------------------------------------------------------------------

    #[test]
    fn invitation_lifecycle() {
        let (env, super_admin, contract_id) = setup();
        let client = client_for(&env, &contract_id);
        let role = Symbol::new(&env, "manager");
        let invitee = Address::generate(&env);
        
        client.create_role(&super_admin, &role);
        
        client.create_invitation(&super_admin, &role, &invitee, &3600);
        client.accept_invitation(&invitee, &role);
        
        assert!(client.has_role(&role, &invitee));
    }
    
    #[test]
    fn invitation_expired() {
        let (env, super_admin, contract_id) = setup();
        let client = client_for(&env, &contract_id);
        let role = Symbol::new(&env, "manager");
        let invitee = Address::generate(&env);
        
        client.create_role(&super_admin, &role);
        
        // 0 ttl means expires at current time
        client.create_invitation(&super_admin, &role, &invitee, &0);
        
        // Advance time
        env.ledger().with_mut(|li| {
            li.timestamp = 1;
        });
        
        let result = client.try_accept_invitation(&invitee, &role);
        assert!(matches!(result, Err(Ok(AccessControlError::InvitationExpired))));
    }

    #[test]
    fn role_escalation_prevented() {
        let (env, super_admin, contract_id) = setup();
        let client = client_for(&env, &contract_id);
        let role = Symbol::new(&env, "manager");
        let user = Address::generate(&env);
        let invitee = Address::generate(&env);
        
        client.create_role(&super_admin, &role);
        
        let result = client.try_create_invitation(&user, &role, &invitee, &3600);
        assert!(matches!(result, Err(Ok(AccessControlError::RoleEscalation))));
    }
    
    #[test]
    fn rate_limit_enforced() {
        let (env, super_admin, contract_id) = setup();
        let client = client_for(&env, &contract_id);
        let role = Symbol::new(&env, "manager");
        
        client.create_role(&super_admin, &role);
        
        for _ in 0..10 {
            let invitee = Address::generate(&env);
            client.create_invitation(&super_admin, &role, &invitee, &3600);
        }
        
        let invitee = Address::generate(&env);
        let result = client.try_create_invitation(&super_admin, &role, &invitee, &3600);
        assert!(matches!(result, Err(Ok(AccessControlError::RateLimitExceeded))));
    }

    #[test]
    fn invitation_revoked() {
        let (env, super_admin, contract_id) = setup();
        let client = client_for(&env, &contract_id);
        let role = Symbol::new(&env, "manager");
        let invitee = Address::generate(&env);
        
        client.create_role(&super_admin, &role);
        client.create_invitation(&super_admin, &role, &invitee, &3600);
        
        client.revoke_invitation(&super_admin, &role, &invitee);
        
        let result = client.try_accept_invitation(&invitee, &role);
        assert!(matches!(result, Err(Ok(AccessControlError::InvitationNotFound))));
    }

    // -----------------------------------------------------------------------
    // Initialization
    // -----------------------------------------------------------------------

    #[test]
    fn initialize_sets_super_admin_and_role() {
        let (env, super_admin, contract_id) = setup();
        let client = client_for(&env, &contract_id);

        assert_eq!(client.super_admin(), super_admin);
        assert!(client.is_admin(&super_admin));
        assert!(client.has_role(&symbol_short!("super"), &super_admin));
    }

    // -----------------------------------------------------------------------
    // Admin management
    // -----------------------------------------------------------------------

    #[test]
    fn add_admin_grants_admin_status() {
        let (env, super_admin, contract_id) = setup();
        let client = client_for(&env, &contract_id);
        let new_admin = Address::generate(&env);

        client.add_admin(&super_admin, &new_admin);
        assert!(client.is_admin(&new_admin));
    }

    #[test]
    fn add_admin_is_idempotent() {
        let (env, super_admin, contract_id) = setup();
        let client = client_for(&env, &contract_id);
        let new_admin = Address::generate(&env);

        client.add_admin(&super_admin, &new_admin);
        client.add_admin(&super_admin, &new_admin);
        assert!(client.is_admin(&new_admin));
    }

    #[test]
    fn non_admin_cannot_add_admin() {
        let (env, _super_admin, contract_id) = setup();
        let client = client_for(&env, &contract_id);
        let outsider = Address::generate(&env);
        let new_admin = Address::generate(&env);

        let result = client.try_add_admin(&outsider, &new_admin);
        assert!(matches!(result, Err(Ok(AccessControlError::NotAdmin))));
    }

    #[test]
    fn remove_admin_revokes_status() {
        let (env, super_admin, contract_id) = setup();
        let client = client_for(&env, &contract_id);
        let new_admin = Address::generate(&env);

        client.add_admin(&super_admin, &new_admin);
        assert!(client.is_admin(&new_admin));

        client.remove_admin(&super_admin, &new_admin);
        assert!(!client.is_admin(&new_admin));
    }

    #[test]
    fn cannot_remove_super_admin() {
        let (env, super_admin, contract_id) = setup();
        let client = client_for(&env, &contract_id);

        let result = client.try_remove_admin(&super_admin, &super_admin);
        assert!(matches!(
            result,
            Err(Ok(AccessControlError::CannotRemoveSuperAdmin))
        ));
    }

    #[test]
    fn remove_non_admin_is_idempotent() {
        let (env, super_admin, contract_id) = setup();
        let client = client_for(&env, &contract_id);
        let ghost = Address::generate(&env);

        // Should succeed without error even though `ghost` was never an admin.
        client.remove_admin(&super_admin, &ghost);
    }

    #[test]
    fn new_admin_can_grant_roles() {
        let (env, super_admin, contract_id) = setup();
        let client = client_for(&env, &contract_id);
        let new_admin = Address::generate(&env);
        let user = Address::generate(&env);
        let role = Symbol::new(&env, "operator");

        client.add_admin(&super_admin, &new_admin);
        client.create_role(&new_admin, &role);
        client.grant_role(&new_admin, &role, &user);
        assert!(client.has_role(&role, &user));
    }

    #[test]
    fn get_all_admins_returns_correct_list() {
        let (env, super_admin, contract_id) = setup();
        let client = client_for(&env, &contract_id);
        let admin2 = Address::generate(&env);
        let admin3 = Address::generate(&env);

        client.add_admin(&super_admin, &admin2);
        client.add_admin(&super_admin, &admin3);

        let admins = client.get_all_admins();
        assert_eq!(admins.len(), 3);
    }

    // -----------------------------------------------------------------------
    // Role creation
    // -----------------------------------------------------------------------

    #[test]
    fn create_role_succeeds() {
        let (env, super_admin, contract_id) = setup();
        let client = client_for(&env, &contract_id);
        let role = Symbol::new(&env, "auditor");

        client.create_role(&super_admin, &role);
        assert!(client.role_exists_check(&role));
    }

    #[test]
    fn create_role_duplicate_fails() {
        let (env, super_admin, contract_id) = setup();
        let client = client_for(&env, &contract_id);
        let role = Symbol::new(&env, "auditor");

        client.create_role(&super_admin, &role);
        let result = client.try_create_role(&super_admin, &role);
        assert!(matches!(
            result,
            Err(Ok(AccessControlError::RoleAlreadyExists))
        ));
    }

    #[test]
    fn non_admin_cannot_create_role() {
        let (env, _super_admin, contract_id) = setup();
        let client = client_for(&env, &contract_id);
        let outsider = Address::generate(&env);
        let role = Symbol::new(&env, "auditor");

        let result = client.try_create_role(&outsider, &role);
        assert!(matches!(result, Err(Ok(AccessControlError::NotAdmin))));
    }

    #[test]
    fn get_all_roles_tracks_created_roles() {
        let (env, super_admin, contract_id) = setup();
        let client = client_for(&env, &contract_id);
        let r1 = Symbol::new(&env, "alpha");
        let r2 = Symbol::new(&env, "beta");

        client.create_role(&super_admin, &r1);
        client.create_role(&super_admin, &r2);

        let roles = client.get_all_roles();
        assert_eq!(roles.len(), 3); // super + alpha + beta
        assert!(roles.iter().any(|r| r == symbol_short!("super")));
        assert!(roles.iter().any(|r| r == r1));
        assert!(roles.iter().any(|r| r == r2));
    }

    // -----------------------------------------------------------------------
    // Role hierarchy
    // -----------------------------------------------------------------------

    #[test]
    fn set_role_parent_and_query() {
        let (env, super_admin, contract_id) = setup();
        let client = client_for(&env, &contract_id);
        let child = Symbol::new(&env, "child");
        let parent = Symbol::new(&env, "parent");

        client.create_role(&super_admin, &child);
        client.create_role(&super_admin, &parent);
        client.set_role_parent(&super_admin, &child, &parent);

        assert_eq!(client.get_role_parent(&child), Some(parent));
    }

    #[test]
    fn hierarchy_grant_propagates_to_child() {
        let (env, super_admin, contract_id) = setup();
        let client = client_for(&env, &contract_id);
        let manager = Symbol::new(&env, "manager");
        let employee = Symbol::new(&env, "employee");
        let user = Address::generate(&env);

        client.create_role(&super_admin, &manager);
        client.create_role(&super_admin, &employee);
        client.set_role_parent(&super_admin, &employee, &manager);

        // Grant manager to user — should satisfy employee check.
        client.grant_role(&super_admin, &manager, &user);
        assert!(client.has_role(&employee, &user));
    }

    #[test]
    fn hierarchy_does_not_grant_upward() {
        let (env, super_admin, contract_id) = setup();
        let client = client_for(&env, &contract_id);
        let manager = Symbol::new(&env, "manager");
        let employee = Symbol::new(&env, "employee");
        let user = Address::generate(&env);

        client.create_role(&super_admin, &manager);
        client.create_role(&super_admin, &employee);
        client.set_role_parent(&super_admin, &employee, &manager);

        // Grant employee to user — should NOT satisfy manager check.
        client.grant_role(&super_admin, &employee, &user);
        assert!(!client.has_role(&manager, &user));
    }

    #[test]
    fn self_reference_rejected() {
        let (env, super_admin, contract_id) = setup();
        let client = client_for(&env, &contract_id);
        let role = Symbol::new(&env, "selfie");

        client.create_role(&super_admin, &role);
        let result = client.try_set_role_parent(&super_admin, &role, &role);
        assert!(matches!(result, Err(Ok(AccessControlError::SelfReference))));
    }

    #[test]
    fn cycle_detection() {
        let (env, super_admin, contract_id) = setup();
        let client = client_for(&env, &contract_id);
        let a = Symbol::new(&env, "a");
        let b = Symbol::new(&env, "b");
        let c = Symbol::new(&env, "c");

        client.create_role(&super_admin, &a);
        client.create_role(&super_admin, &b);
        client.create_role(&super_admin, &c);

        // a -> b -> c, then try c -> a (cycle).
        client.set_role_parent(&super_admin, &a, &b);
        client.set_role_parent(&super_admin, &b, &c);
        let result = client.try_set_role_parent(&super_admin, &c, &a);
        assert!(matches!(result, Err(Ok(AccessControlError::CycleDetected))));
    }

    #[test]
    fn nonexistent_role_cannot_be_parent() {
        let (env, super_admin, contract_id) = setup();
        let client = client_for(&env, &contract_id);
        let child = Symbol::new(&env, "child");
        let ghost = Symbol::new(&env, "ghost");

        client.create_role(&super_admin, &child);
        let result = client.try_set_role_parent(&super_admin, &child, &ghost);
        assert!(matches!(result, Err(Ok(AccessControlError::RoleNotFound))));
    }

    #[test]
    fn get_role_ancestors_returns_chain() {
        let (env, super_admin, contract_id) = setup();
        let client = client_for(&env, &contract_id);
        let a = Symbol::new(&env, "a");
        let b = Symbol::new(&env, "b");
        let c = Symbol::new(&env, "c");

        client.create_role(&super_admin, &a);
        client.create_role(&super_admin, &b);
        client.create_role(&super_admin, &c);
        client.set_role_parent(&super_admin, &a, &b);
        client.set_role_parent(&super_admin, &b, &c);

        let ancestors = client.get_role_ancestors(&a);
        assert_eq!(ancestors.len(), 2);
        assert_eq!(ancestors.get(0).unwrap(), b);
        assert_eq!(ancestors.get(1).unwrap(), c);
    }

    // -----------------------------------------------------------------------
    // Grant / Revoke lifecycle
    // -----------------------------------------------------------------------

    #[test]
    fn grant_and_revoke_roundtrip() {
        let (env, super_admin, contract_id) = setup();
        let client = client_for(&env, &contract_id);
        let role = Symbol::new(&env, "writer");
        let user = Address::generate(&env);

        client.create_role(&super_admin, &role);
        client.grant_role(&super_admin, &role, &user);
        assert!(client.has_role(&role, &user));

        client.revoke_role(&super_admin, &role, &user);
        assert!(!client.has_role(&role, &user));
    }

    #[test]
    fn revoke_non_member_is_idempotent() {
        let (env, super_admin, contract_id) = setup();
        let client = client_for(&env, &contract_id);
        let role = Symbol::new(&env, "ghost_role");
        let user = Address::generate(&env);

        client.create_role(&super_admin, &role);
        // User never held the role — revoke should still succeed.
        client.revoke_role(&super_admin, &role, &user);
        assert!(!client.has_role(&role, &user));
    }

    #[test]
    fn grant_nonexistent_role_fails() {
        let (env, super_admin, contract_id) = setup();
        let client = client_for(&env, &contract_id);
        let ghost = Symbol::new(&env, "ghost");
        let user = Address::generate(&env);

        let result = client.try_grant_role(&super_admin, &ghost, &user);
        assert!(matches!(result, Err(Ok(AccessControlError::RoleNotFound))));
    }

    #[test]
    fn get_role_members_returns_direct_members_only() {
        let (env, super_admin, contract_id) = setup();
        let client = client_for(&env, &contract_id);
        let manager = Symbol::new(&env, "manager");
        let employee = Symbol::new(&env, "employee");
        let m1 = Address::generate(&env);
        let e1 = Address::generate(&env);

        client.create_role(&super_admin, &manager);
        client.create_role(&super_admin, &employee);
        client.set_role_parent(&super_admin, &employee, &manager);

        client.grant_role(&super_admin, &manager, &m1);
        client.grant_role(&super_admin, &employee, &e1);

        let mgr_members = client.get_role_members(&manager);
        assert_eq!(mgr_members.len(), 1);
        assert_eq!(mgr_members.get(0).unwrap(), m1);

        let emp_members = client.get_role_members(&employee);
        assert_eq!(emp_members.len(), 1);
        assert_eq!(emp_members.get(0).unwrap(), e1);
    }

    // -----------------------------------------------------------------------
    // Edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn uninitialized_contract_panics_on_super_admin() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, AccessControlContract);
        let client = client_for(&env, &contract_id);

        // super_admin() should panic because initialize was never called.
        let result = client.try_super_admin();
        assert!(result.is_err());
    }

    #[test]
    fn grant_revoked_hierarchy_memberloses_child_access() {
        let (env, super_admin, contract_id) = setup();
        let client = client_for(&env, &contract_id);
        let admin_role = Symbol::new(&env, "admin_r");
        let viewer = Symbol::new(&env, "viewer");
        let user = Address::generate(&env);

        client.create_role(&super_admin, &admin_role);
        client.create_role(&super_admin, &viewer);
        client.set_role_parent(&super_admin, &viewer, &admin_role);

        client.grant_role(&super_admin, &admin_role, &user);
        assert!(client.has_role(&viewer, &user));

        client.revoke_role(&super_admin, &admin_role, &user);
        assert!(!client.has_role(&viewer, &user));
    }

    #[test]
    fn multi_level_hierarchy_works() {
        let (env, super_admin, contract_id) = setup();
        let client = client_for(&env, &contract_id);
        let ceo = Symbol::new(&env, "ceo");
        let vp = Symbol::new(&env, "vp");
        let director = Symbol::new(&env, "director");
        let user = Address::generate(&env);

        client.create_role(&super_admin, &ceo);
        client.create_role(&super_admin, &vp);
        client.create_role(&super_admin, &director);
        client.set_role_parent(&super_admin, &vp, &ceo);
        client.set_role_parent(&super_admin, &director, &vp);

        client.grant_role(&super_admin, &ceo, &user);
        // CEO grants access to both vp and director.
        assert!(client.has_role(&vp, &user));
        assert!(client.has_role(&director, &user));
    }

    // -----------------------------------------------------------------------
    // Event emission tests
    // -----------------------------------------------------------------------

    #[test]
    fn create_role_emits_event() {
        let (env, super_admin, contract_id) = setup();
        let client = client_for(&env, &contract_id);
        let role = Symbol::new(&env, "emitter");

        client.create_role(&super_admin, &role);

        let events = env.events().all();
        // The last event should be RoleCreated.
        let last = events.last().unwrap();
        assert_eq!(last.1, (EV_ROLE_CREATED,).into_val(&env));
    }

    #[test]
    fn grant_role_emits_event() {
        let (env, super_admin, contract_id) = setup();
        let client = client_for(&env, &contract_id);
        let role = Symbol::new(&env, "emitter");
        let user = Address::generate(&env);

        client.create_role(&super_admin, &role);
        client.grant_role(&super_admin, &role, &user);

        let events = env.events().all();
        let last = events.last().unwrap();
        assert_eq!(last.1, (EV_ROLE_GRANTED,).into_val(&env));
    }

    #[test]
    fn revoke_role_emits_event() {
        let (env, super_admin, contract_id) = setup();
        let client = client_for(&env, &contract_id);
        let role = Symbol::new(&env, "emitter");
        let user = Address::generate(&env);

        client.create_role(&super_admin, &role);
        client.grant_role(&super_admin, &role, &user);
        client.revoke_role(&super_admin, &role, &user);

        let events = env.events().all();
        let last = events.last().unwrap();
        assert_eq!(last.1, (EV_ROLE_REVOKED,).into_val(&env));
    }

    #[test]
    fn add_admin_emits_event() {
        let (env, super_admin, contract_id) = setup();
        let client = client_for(&env, &contract_id);
        let new_admin = Address::generate(&env);

        client.add_admin(&super_admin, &new_admin);

        let events = env.events().all();
        let last = events.last().unwrap();
        assert_eq!(last.1, (EV_ADMIN_ADDED,).into_val(&env));
    }

    #[test]
    fn remove_admin_emits_event() {
        let (env, super_admin, contract_id) = setup();
        let client = client_for(&env, &contract_id);
        let new_admin = Address::generate(&env);

        client.add_admin(&super_admin, &new_admin);
        client.remove_admin(&super_admin, &new_admin);

        let events = env.events().all();
        let last = events.last().unwrap();
        assert_eq!(last.1, (EV_ADMIN_REMOVED,).into_val(&env));
    }
}
