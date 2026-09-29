//! # Access-control permission matrix
//!
//! One authoritative table mapping every privileged action exposed by the
//! access-control contract to the capability it needs
//! ([`shared::auth::Permission`]), the role that owns it, and the scope it may
//! be exercised in.
//!
//! Entry points call [`require_action`] instead of hand-rolling their own
//! admin check, so a new privileged action adds one row here rather than a new
//! bespoke guard. [`ActionScope`] records how narrow the authority is:
//! deployment-wide for maintainer actions, confined to a single role for
//! member management, or confined to records the caller owns.

use shared::auth::{self, Permission, Role};
use soroban_sdk::{contracttype, Address, Env, Symbol};

use crate::{has_role_recursive, is_admin_internal, AccessControlError};

/// A privileged action exposed by the access-control contract.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Action {
    /// Register or remove a maintainer (admin) of this contract.
    ManageMaintainers,
    /// Create a role or change the role hierarchy.
    ConfigureRoleRegistry,
    /// Grant or revoke a role on an address.
    AssignRoles,
    /// Invite an address to a specific role.
    InviteMember(Symbol),
    /// Cancel a pending invitation owned by the given inviter.
    CancelInvitation(Address),
    /// Read the maintainer-only audit trail.
    ReadAuditTrail,
}

/// How widely an action's authority extends.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ActionScope {
    /// Deployment-wide. Only maintainer authority satisfies it.
    Global,
    /// Confined to one role. A maintainer, or a holder of that role,
    /// satisfies it.
    RoleScoped(Symbol),
    /// Confined to records owned by the given address. A maintainer, or the
    /// owner, satisfies it.
    OwnerScoped(Address),
}

/// The capability, owning role, and scope a privileged action requires.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActionPolicy {
    /// The shared capability the action consumes.
    pub permission: Permission,
    /// The role that owns the capability.
    pub role: Role,
    /// The breadth within which the action may be exercised.
    pub scope: ActionScope,
}

/// Returns the single-source-of-truth policy for `action`.
pub fn policy_for(action: &Action) -> ActionPolicy {
    match action {
        Action::ManageMaintainers => ActionPolicy {
            permission: Permission::ManageRoles,
            role: Role::Admin,
            scope: ActionScope::Global,
        },
        Action::ConfigureRoleRegistry => ActionPolicy {
            permission: Permission::ManageConfiguration,
            role: Role::Admin,
            scope: ActionScope::Global,
        },
        Action::AssignRoles => ActionPolicy {
            permission: Permission::ManageRoles,
            role: Role::Admin,
            scope: ActionScope::Global,
        },
        Action::InviteMember(role) => ActionPolicy {
            permission: Permission::ManageRoles,
            role: Role::Admin,
            scope: ActionScope::RoleScoped(role.clone()),
        },
        Action::CancelInvitation(inviter) => ActionPolicy {
            permission: Permission::ManageRoles,
            role: Role::Admin,
            scope: ActionScope::OwnerScoped(inviter.clone()),
        },
        Action::ReadAuditTrail => ActionPolicy {
            permission: Permission::ReadAuditTrail,
            role: Role::Admin,
            scope: ActionScope::Global,
        },
    }
}

/// Maintainer authority: this contract's admin registry, or the shared
/// capability model (which also honours the stored-admin fallback).
fn has_maintainer_authority(env: &Env, caller: &Address, permission: Permission) -> bool {
    is_admin_internal(env, caller) || auth::has_permission(env, caller, permission)
}

/// Returns `true` when `caller` may perform `action` under its scope.
pub fn has_action_permission(env: &Env, caller: &Address, action: &Action) -> bool {
    let policy = policy_for(action);
    match policy.scope {
        ActionScope::Global => has_maintainer_authority(env, caller, policy.permission),
        ActionScope::RoleScoped(role) => {
            has_maintainer_authority(env, caller, policy.permission)
                || has_role_recursive(env, &role, caller)
        }
        ActionScope::OwnerScoped(owner) => {
            has_maintainer_authority(env, caller, policy.permission) || owner == *caller
        }
    }
}

/// Verifies `caller` may perform `action` and has authorized the call.
///
/// A missing deployment-wide authority fails with
/// [`AccessControlError::NotAdmin`]; a caller outside the scope of a
/// role- or owner-scoped action fails with
/// [`AccessControlError::RoleEscalation`].
pub fn require_action(
    env: &Env,
    caller: &Address,
    action: &Action,
) -> Result<(), AccessControlError> {
    if !has_action_permission(env, caller, action) {
        return Err(match policy_for(action).scope {
            ActionScope::Global => AccessControlError::NotAdmin,
            ActionScope::RoleScoped(_) | ActionScope::OwnerScoped(_) => {
                AccessControlError::RoleEscalation
            }
        });
    }
    caller.require_auth();
    Ok(())
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use crate::{AccessControlContract, AccessControlContractClient, AccessControlError};
    use soroban_sdk::symbol_short;
    use soroban_sdk::testutils::Ledger as _;
    use soroban_sdk::testutils::Address as _;

    fn setup() -> (Env, Address, Address) {
        let env = Env::default();
        env.mock_all_auths();
        let admin = Address::generate(&env);
        let contract_id = env.register_contract(None, AccessControlContract);
        AccessControlContractClient::new(&env, &contract_id).initialize(&admin);
        (env, admin, contract_id)
    }

    fn client_for<'a>(env: &'a Env, contract_id: &Address) -> AccessControlContractClient<'a> {
        AccessControlContractClient::new(env, contract_id)
    }

    // -----------------------------------------------------------------------
    // The matrix itself: every action has exactly one capability and scope.
    // -----------------------------------------------------------------------

    #[test]
    fn every_action_maps_to_one_capability_and_scope() {
        let env = Env::default();
        let role = symbol_short!("manager");
        let inviter = Address::generate(&env);

        let matrix: [(Action, Permission, ActionScope); 6] = [
            (
                Action::ManageMaintainers,
                Permission::ManageRoles,
                ActionScope::Global,
            ),
            (
                Action::ConfigureRoleRegistry,
                Permission::ManageConfiguration,
                ActionScope::Global,
            ),
            (
                Action::AssignRoles,
                Permission::ManageRoles,
                ActionScope::Global,
            ),
            (
                Action::InviteMember(role.clone()),
                Permission::ManageRoles,
                ActionScope::RoleScoped(role.clone()),
            ),
            (
                Action::CancelInvitation(inviter.clone()),
                Permission::ManageRoles,
                ActionScope::OwnerScoped(inviter.clone()),
            ),
            (
                Action::ReadAuditTrail,
                Permission::ReadAuditTrail,
                ActionScope::Global,
            ),
        ];

        for (action, permission, scope) in matrix {
            let policy = policy_for(&action);
            assert_eq!(policy.permission, permission, "capability for {action:?}");
            assert_eq!(policy.scope, scope, "scope for {action:?}");
            // Every privileged action in this contract is owned by Admin; the
            // specialized manager roles are delegated at the call site.
            assert_eq!(policy.role, Role::Admin, "owner role for {action:?}");
        }
    }

    #[test]
    fn only_invitations_are_narrower_than_deployment_wide() {
        let env = Env::default();
        let role = symbol_short!("manager");
        let inviter = Address::generate(&env);

        let global = [
            Action::ManageMaintainers,
            Action::ConfigureRoleRegistry,
            Action::AssignRoles,
            Action::ReadAuditTrail,
        ];
        for action in global {
            assert_eq!(policy_for(&action).scope, ActionScope::Global);
        }
        assert_eq!(
            policy_for(&Action::InviteMember(role.clone())).scope,
            ActionScope::RoleScoped(role)
        );
        assert_eq!(
            policy_for(&Action::CancelInvitation(inviter.clone())).scope,
            ActionScope::OwnerScoped(inviter)
        );
    }

    // -----------------------------------------------------------------------
    // Allowed: the registered maintainer satisfies every action.
    // -----------------------------------------------------------------------

    #[test]
    fn maintainer_satisfies_every_action() {
        let (env, admin, contract_id) = setup();
        let client = client_for(&env, &contract_id);
        let role = symbol_short!("manager");
        let user = Address::generate(&env);
        let invitee = Address::generate(&env);

        client.create_role(&admin, &role);
        client.set_role_parent(&admin, &role, &symbol_short!("super"));
        client.grant_role(&admin, &role, &user);
        client.create_invitation(&admin, &role, &invitee, &3600);
        client.revoke_invitation(&admin, &role, &invitee);
        client.add_admin(&admin, &user);
        client.remove_admin(&admin, &user);
        assert!(!client.audit_trail(&admin, &10).is_empty());
    }

    // -----------------------------------------------------------------------
    // Denied: an outsider holds no capability at all.
    // -----------------------------------------------------------------------

    #[test]
    fn outsider_is_denied_every_global_action() {
        let (env, _admin, contract_id) = setup();
        let client = client_for(&env, &contract_id);
        let outsider = Address::generate(&env);
        let role = symbol_short!("manager");
        let user = Address::generate(&env);

        let denied = [
            client.try_add_admin(&outsider, &user),
            client.try_remove_admin(&outsider, &user),
            client.try_create_role(&outsider, &role),
            client.try_set_role_parent(&outsider, &role, &symbol_short!("super")),
            client.try_grant_role(&outsider, &role, &user),
            client.try_revoke_role(&outsider, &role, &user),
        ];
        for result in denied {
            assert!(
                matches!(result, Err(Ok(AccessControlError::NotAdmin))),
                "outsider must be denied with NotAdmin"
            );
        }
        assert!(client.try_audit_trail(&outsider, &10).is_err());
    }

    // -----------------------------------------------------------------------
    // Scope-limited: authority over one role does not leak into another.
    // -----------------------------------------------------------------------

    #[test]
    fn role_holder_may_invite_only_into_their_own_role() {
        let (env, admin, contract_id) = setup();
        let client = client_for(&env, &contract_id);
        let managed = symbol_short!("manager");
        let other = symbol_short!("ops");
        let holder = Address::generate(&env);
        let invitee = Address::generate(&env);

        client.create_role(&admin, &managed);
        client.create_role(&admin, &other);
        client.grant_role(&admin, &managed, &holder);

        // Allowed: the holder manages membership of the role they hold.
        client.create_invitation(&holder, &managed, &invitee, &3600);

        // Denied: the same holder has no authority over a role they do not
        // hold.
        let result = client.try_create_invitation(&holder, &other, &invitee, &3600);
        assert!(matches!(
            result,
            Err(Ok(AccessControlError::RoleEscalation))
        ));
    }

    #[test]
    fn inviter_may_cancel_only_their_own_invitation() {
        let (env, admin, contract_id) = setup();
        let client = client_for(&env, &contract_id);
        let role = symbol_short!("manager");
        let inviter = Address::generate(&env);
        let peer = Address::generate(&env);
        let invitee = Address::generate(&env);

        client.create_role(&admin, &role);
        client.grant_role(&admin, &role, &inviter);
        client.grant_role(&admin, &role, &peer);
        client.create_invitation(&inviter, &role, &invitee, &3600);

        // Denied: a peer with the same role does not own the invitation.
        let result = client.try_revoke_invitation(&peer, &role, &invitee);
        assert!(matches!(
            result,
            Err(Ok(AccessControlError::RoleEscalation))
        ));

        // Allowed: the original inviter owns it.
        client.revoke_invitation(&inviter, &role, &invitee);
        assert!(matches!(
            client.try_accept_invitation(&invitee, &role),
            Err(Ok(AccessControlError::InvitationNotFound))
        ));
    }

    #[test]
    fn maintainer_may_cancel_any_invitation() {
        let (env, admin, contract_id) = setup();
        let client = client_for(&env, &contract_id);
        let role = symbol_short!("manager");
        let inviter = Address::generate(&env);
        let invitee = Address::generate(&env);

        client.create_role(&admin, &role);
        client.grant_role(&admin, &role, &inviter);
        client.create_invitation(&inviter, &role, &invitee, &3600);

        client.revoke_invitation(&admin, &role, &invitee);
        assert!(matches!(
            client.try_accept_invitation(&invitee, &role),
            Err(Ok(AccessControlError::InvitationNotFound))
        ));
    }

    // -----------------------------------------------------------------------
    // Expired: an invitation past its deadline cannot be accepted, and the
    // inviter's authority over it lapses with it.
    // -----------------------------------------------------------------------

    #[test]
    fn expired_invitation_cannot_be_accepted() {
        let (env, admin, contract_id) = setup();
        let client = client_for(&env, &contract_id);
        let role = symbol_short!("manager");
        let invitee = Address::generate(&env);

        client.create_role(&admin, &role);
        client.create_invitation(&admin, &role, &invitee, &3600);

        // Advance the ledger past the invitation's expiry.
        env.ledger().with_mut(|l| {
            l.timestamp += 3601;
        });

        let result = client.try_accept_invitation(&invitee, &role);
        assert!(
            matches!(result, Err(Ok(AccessControlError::InvitationExpired))),
            "expired invitation must be rejected"
        );
    }

    #[test]
    fn expired_invitation_cannot_be_revoked_by_non_owner() {
        let (env, admin, contract_id) = setup();
        let client = client_for(&env, &contract_id);
        let role = symbol_short!("manager");
        let inviter = Address::generate(&env);
        let peer = Address::generate(&env);
        let invitee = Address::generate(&env);

        client.create_role(&admin, &role);
        client.grant_role(&admin, &role, &inviter);
        client.grant_role(&admin, &role, &peer);
        client.create_invitation(&inviter, &role, &invitee, &3600);

        env.ledger().with_mut(|l| {
            l.timestamp += 3601;
        });

        // Denied: expiry does not widen authority; a peer still cannot
        // revoke an invitation they do not own.
        let result = client.try_revoke_invitation(&peer, &role, &invitee);
        assert!(matches!(
            result,
            Err(Ok(AccessControlError::RoleEscalation))
        ));
    }

    // -----------------------------------------------------------------------
    // Revoked: an actor whose role was revoked loses every scoped privilege.
    // -----------------------------------------------------------------------

    #[test]
    fn revoked_role_holder_is_denied_scoped_actions() {
        let (env, admin, contract_id) = setup();
        let client = client_for(&env, &contract_id);
        let role = symbol_short!("manager");
        let holder = Address::generate(&env);
        let invitee = Address::generate(&env);

        client.create_role(&admin, &role);
        client.grant_role(&admin, &role, &holder);

        // Allowed before revocation.
        client.create_invitation(&holder, &role, &invitee, &3600);

        // Revoke the holder's role.
        client.revoke_role(&admin, &role, &holder);

        // Denied: the revoked holder can no longer invite into the role.
        let result = client.try_create_invitation(&holder, &role, &invitee, &3600);
        assert!(
            matches!(result, Err(Ok(AccessControlError::RoleEscalation))),
            "revoked role holder must be denied scoped actions"
        );
    }

    #[test]
    fn revoked_maintainer_is_denied_global_actions() {
        let (env, admin, contract_id) = setup();
        let client = client_for(&env, &contract_id);
        let maintainer = Address::generate(&env);
        let role = symbol_short!("manager");
        let user = Address::generate(&env);

        client.add_admin(&admin, &maintainer);

        // Allowed before revocation.
        client.create_role(&maintainer, &role);

        // Revoke maintainer authority.
        client.remove_admin(&admin, &maintainer);

        // Denied: the revoked maintainer can no longer perform global
        // actions.
        let result = client.try_grant_role(&maintainer, &role, &user);
        assert!(
            matches!(result, Err(Ok(AccessControlError::NotAdmin))),
            "revoked maintainer must be denied global actions"
        );
    }

    // -----------------------------------------------------------------------
    // Unauthorized: an actor with no role at all is denied scoped actions.
    // -----------------------------------------------------------------------

    #[test]
    fn actor_without_role_is_denied_scoped_actions() {
        let (env, admin, contract_id) = setup();
        let client = client_for(&env, &contract_id);
        let role = symbol_short!("manager");
        let outsider = Address::generate(&env);
        let invitee = Address::generate(&env);

        client.create_role(&admin, &role);

        // Denied: an outsider holds no role, so cannot invite into it.
        let result = client.try_create_invitation(&outsider, &role, &invitee, &3600);
        assert!(matches!(
            result,
            Err(Ok(AccessControlError::RoleEscalation))
        ));

        // Denied: an outsider cannot cancel an invitation they do not own.
        let result = client.try_revoke_invitation(&outsider, &role, &invitee);
        assert!(matches!(
            result,
            Err(Ok(AccessControlError::RoleEscalation))
        ));
    }
}
