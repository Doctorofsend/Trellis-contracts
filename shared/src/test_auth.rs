#![cfg(test)]

extern crate std;

use soroban_sdk::{
    contract, contractimpl,
    testutils::{Address as _, Ledger as _},
    Address, Env,
};

use crate::{
    auth::{
        accept_ownership_transfer, cancel_ownership_transfer, get_admin,
        get_pending_ownership_transfer, grant_role, has_permission, has_role, initialize_admin,
        propose_ownership_transfer, require_admin, require_not_paused, require_permission,
        require_role, revoke_role, role_for_permission, set_admin, PendingOwnershipTransfer,
        Permission, Role,
    },
    errors::Error,
    storage::set_paused,
};

#[contract]
pub struct DummyAuthContract;

#[contractimpl]
impl DummyAuthContract {
    pub fn noop(_env: Env) {}
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Creates a fresh environment, registers a dummy contract, and sets a random admin address.
/// Returns (env, contract_id, admin).
fn setup() -> (Env, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let contract_id = env.register_contract(None, DummyAuthContract);
    env.as_contract(&contract_id, || {
        set_admin(&env, &admin);
    });
    (env, contract_id, admin)
}

// ---------------------------------------------------------------------------
// require_admin
// ---------------------------------------------------------------------------

#[test]
fn test_require_admin_succeeds_for_admin() {
    let (env, contract_id, admin) = setup();
    env.as_contract(&contract_id, || {
        assert_eq!(require_admin(&env, &admin), Ok(()));
    });
}

#[test]
fn test_require_admin_fails_for_non_admin() {
    let (env, contract_id, _admin) = setup();
    let other = Address::generate(&env);
    env.as_contract(&contract_id, || {
        assert_eq!(require_admin(&env, &other), Err(Error::Unauthorized));
    });
}

// ---------------------------------------------------------------------------
// grant_role / revoke_role — admin gating
// ---------------------------------------------------------------------------

#[test]
fn test_grant_role_by_admin_succeeds() {
    let (env, contract_id, admin) = setup();
    let user = Address::generate(&env);
    env.as_contract(&contract_id, || {
        let result = grant_role(&env, &admin, &user, Role::Upgrader);
        assert_eq!(result, Ok(()));
        assert!(has_role(&env, &user, Role::Upgrader));
    });
}

#[test]
fn test_grant_role_by_non_admin_fails() {
    let (env, contract_id, _admin) = setup();
    let attacker = Address::generate(&env);
    let user = Address::generate(&env);
    env.as_contract(&contract_id, || {
        let result = grant_role(&env, &attacker, &user, Role::Upgrader);
        assert_eq!(result, Err(Error::Unauthorized));
        assert!(!has_role(&env, &user, Role::Upgrader));
    });
}

#[test]
fn test_revoke_role_by_admin_succeeds() {
    let (env, contract_id, admin) = setup();
    let user = Address::generate(&env);
    env.as_contract(&contract_id, || {
        grant_role(&env, &admin, &user, Role::TreasuryManager).unwrap();
        assert!(has_role(&env, &user, Role::TreasuryManager));
    });
    env.as_contract(&contract_id, || {
        let result = revoke_role(&env, &admin, &user, Role::TreasuryManager);
        assert_eq!(result, Ok(()));
        assert!(!has_role(&env, &user, Role::TreasuryManager));
    });
}

#[test]
fn test_revoke_role_by_non_admin_fails() {
    let (env, contract_id, admin) = setup();
    let attacker = Address::generate(&env);
    let user = Address::generate(&env);
    env.as_contract(&contract_id, || {
        grant_role(&env, &admin, &user, Role::Pauser).unwrap();

        let result = revoke_role(&env, &attacker, &user, Role::Pauser);
        assert_eq!(result, Err(Error::Unauthorized));
        // Role should still be present after the failed revocation.
        assert!(has_role(&env, &user, Role::Pauser));
    });
}

// ---------------------------------------------------------------------------
// require_role
// ---------------------------------------------------------------------------

#[test]
fn test_require_role_passes_when_role_held() {
    let (env, contract_id, admin) = setup();
    let user = Address::generate(&env);
    env.as_contract(&contract_id, || {
        grant_role(&env, &admin, &user, Role::Upgrader).unwrap();
        assert_eq!(require_role(&env, &user, Role::Upgrader), Ok(()));
    });
}

#[test]
fn test_require_role_fails_when_role_not_held() {
    let (env, contract_id, _admin) = setup();
    let user = Address::generate(&env);
    env.as_contract(&contract_id, || {
        assert_eq!(
            require_role(&env, &user, Role::TreasuryManager),
            Err(Error::Unauthorized)
        );
    });
}

#[test]
fn test_require_role_fails_after_role_revoked() {
    let (env, contract_id, admin) = setup();
    let user = Address::generate(&env);
    env.as_contract(&contract_id, || {
        grant_role(&env, &admin, &user, Role::Pauser).unwrap();
    });
    env.as_contract(&contract_id, || {
        revoke_role(&env, &admin, &user, Role::Pauser).unwrap();
    });
    env.as_contract(&contract_id, || {
        assert_eq!(
            require_role(&env, &user, Role::Pauser),
            Err(Error::Unauthorized)
        );
    });
}

#[test]
fn test_roles_are_independent_per_role_variant() {
    let (env, contract_id, admin) = setup();
    let user = Address::generate(&env);
    env.as_contract(&contract_id, || {
        grant_role(&env, &admin, &user, Role::Upgrader).unwrap();
        // Holding Upgrader does not grant TreasuryManager.
        assert_eq!(
            require_role(&env, &user, Role::TreasuryManager),
            Err(Error::Unauthorized)
        );
    });
}

// ---------------------------------------------------------------------------
// require_not_paused
// ---------------------------------------------------------------------------

#[test]
fn test_require_not_paused_passes_when_active() {
    let (env, contract_id, _admin) = setup();
    env.as_contract(&contract_id, || {
        // Default state: not paused.
        assert_eq!(require_not_paused(&env), Ok(()));
    });
}

#[test]
fn test_require_not_paused_blocks_when_paused() {
    let (env, contract_id, _admin) = setup();
    env.as_contract(&contract_id, || {
        set_paused(&env, true);
        assert_eq!(require_not_paused(&env), Err(Error::ContractPaused));
    });
}

#[test]
fn test_require_not_paused_passes_after_resume() {
    let (env, contract_id, _admin) = setup();
    env.as_contract(&contract_id, || {
        set_paused(&env, true);
        // Resume the contract.
        set_paused(&env, false);
        assert_eq!(require_not_paused(&env), Ok(()));
    });
}

// ---------------------------------------------------------------------------
// get_admin
// ---------------------------------------------------------------------------

#[test]
fn test_get_admin_returns_set_admin() {
    let (env, contract_id, admin) = setup();
    env.as_contract(&contract_id, || {
        assert_eq!(get_admin(&env), admin);
    });
}

#[test]
fn permissions_map_to_their_central_roles() {
    assert_eq!(
        role_for_permission(Permission::UseOwnResources),
        Role::EndUser
    );
    assert_eq!(
        role_for_permission(Permission::ManageConfiguration),
        Role::Admin
    );
    assert_eq!(role_for_permission(Permission::ManageRoles), Role::Admin);
    assert_eq!(
        role_for_permission(Permission::TreasuryOperations),
        Role::TreasuryManager
    );
    assert_eq!(
        role_for_permission(Permission::PauseContracts),
        Role::Pauser
    );
    assert_eq!(
        role_for_permission(Permission::ReferralConfiguration),
        Role::ReferralManager
    );
    assert_eq!(
        role_for_permission(Permission::SubmitOracle),
        Role::OracleSigner
    );
    assert_eq!(
        role_for_permission(Permission::UpgradeContracts),
        Role::Upgrader
    );
    assert_eq!(role_for_permission(Permission::ReadAuditTrail), Role::Admin);
    assert_eq!(
        role_for_permission(Permission::ServiceOperation),
        Role::ServiceActor
    );
}

#[test]
fn named_permission_requires_a_granted_role_and_authentication() {
    let (env, contract_id, admin) = setup();
    let service = Address::generate(&env);
    env.as_contract(&contract_id, || {
        assert!(!has_permission(
            &env,
            &service,
            Permission::ServiceOperation
        ));
        assert_eq!(
            require_permission(&env, &service, Permission::ServiceOperation),
            Err(Error::Unauthorized)
        );
    });
    env.as_contract(&contract_id, || {
        grant_role(&env, &admin, &service, Role::ServiceActor).unwrap();
    });
    env.as_contract(&contract_id, || {
        assert!(has_permission(&env, &service, Permission::ServiceOperation));
        assert_eq!(
            require_permission(&env, &service, Permission::ServiceOperation),
            Ok(())
        );
    });
    env.as_contract(&contract_id, || {
        revoke_role(&env, &admin, &service, Role::ServiceActor).unwrap();
    });
    env.as_contract(&contract_id, || {
        assert_eq!(
            require_permission(&env, &service, Permission::ServiceOperation),
            Err(Error::Unauthorized)
        );
    });
}

#[test]
fn admin_permissions_preserve_existing_maintainer_access() {
    let (env, contract_id, admin) = setup();
    let delegated_admin = Address::generate(&env);
    env.as_contract(&contract_id, || {
        assert!(has_permission(&env, &admin, Permission::TreasuryOperations));
        assert_eq!(
            require_permission(&env, &admin, Permission::UpgradeContracts),
            Ok(())
        );
    });

    env.as_contract(&contract_id, || {
        grant_role(&env, &admin, &delegated_admin, Role::Admin).unwrap();
    });

    env.as_contract(&contract_id, || {
        assert!(has_permission(
            &env,
            &delegated_admin,
            Permission::PauseContracts
        ));
        assert_eq!(
            require_permission(&env, &delegated_admin, Permission::ReferralConfiguration),
            Ok(())
        );

        assert!(!has_permission(&env, &admin, Permission::ServiceOperation));
        assert!(!has_permission(&env, &admin, Permission::SubmitOracle));
    });
}

#[test]
fn initialize_admin_requires_signature_and_cannot_be_repeated() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let contract_id = env.register_contract(None, DummyAuthContract);

    env.as_contract(&contract_id, || {
        assert_eq!(initialize_admin(&env, &admin), Ok(()));
        assert_eq!(get_admin(&env), admin);
        assert!(has_role(&env, &admin, Role::Admin));
        assert_eq!(
            initialize_admin(&env, &admin),
            Err(Error::AlreadyInitialized)
        );
    });
}

fn setup_with_admin_role() -> (Env, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let contract_id = env.register_contract(None, DummyAuthContract);
    env.as_contract(&contract_id, || {
        initialize_admin(&env, &admin).unwrap();
    });
    (env, contract_id, admin)
}

// ---------------------------------------------------------------------------
// Ownership transfer tests (Issue #150)
// ---------------------------------------------------------------------------

#[test]
fn test_ownership_transfer_lifecycle() {
    let (env, contract_id, current_owner) = setup_with_admin_role();
    let new_owner = Address::generate(&env);
    env.ledger().set_timestamp(100);

    // 1. Propose transfer
    env.as_contract(&contract_id, || {
        assert_eq!(get_admin(&env), current_owner);
        assert!(has_role(&env, &current_owner, Role::Admin));
        assert!(!has_role(&env, &new_owner, Role::Admin));
        assert_eq!(get_pending_ownership_transfer(&env), None);

        propose_ownership_transfer(&env, &current_owner, &new_owner, 200).unwrap();

        let pending = get_pending_ownership_transfer(&env).unwrap();
        assert_eq!(pending.current_owner, current_owner);
        assert_eq!(pending.target_owner, new_owner);
        assert_eq!(pending.expiry, 200);

        // Crucial: ownership does not change until accepted
        assert_eq!(get_admin(&env), current_owner);
        assert!(has_role(&env, &current_owner, Role::Admin));
        assert!(!has_role(&env, &new_owner, Role::Admin));
    });

    // 2. Accept transfer by new owner
    env.ledger().set_timestamp(150);
    env.as_contract(&contract_id, || {
        accept_ownership_transfer(&env, &new_owner).unwrap();

        // Ownership has changed
        assert_eq!(get_admin(&env), new_owner);
        assert!(has_role(&env, &new_owner, Role::Admin));
        assert!(!has_role(&env, &current_owner, Role::Admin));
        assert_eq!(get_pending_ownership_transfer(&env), None);
    });
}

#[test]
fn test_ownership_transfer_cancel() {
    let (env, contract_id, current_owner) = setup_with_admin_role();
    let target = Address::generate(&env);
    env.ledger().set_timestamp(100);

    env.as_contract(&contract_id, || {
        propose_ownership_transfer(&env, &current_owner, &target, 200).unwrap();
        assert!(get_pending_ownership_transfer(&env).is_some());
    });

    env.as_contract(&contract_id, || {
        cancel_ownership_transfer(&env, &current_owner).unwrap();
        assert_eq!(get_pending_ownership_transfer(&env), None);
        assert_eq!(get_admin(&env), current_owner);
    });

    // Target cannot accept after cancellation
    env.as_contract(&contract_id, || {
        assert_eq!(
            accept_ownership_transfer(&env, &target),
            Err(Error::NoPendingTransfer)
        );
    });
}

#[test]
fn test_ownership_transfer_expiry_rejected() {
    let (env, contract_id, current_owner) = setup_with_admin_role();
    let target = Address::generate(&env);
    env.ledger().set_timestamp(100);

    env.as_contract(&contract_id, || {
        propose_ownership_transfer(&env, &current_owner, &target, 200).unwrap();
    });

    // Advance beyond expiry
    env.ledger().set_timestamp(201);

    env.as_contract(&contract_id, || {
        assert_eq!(
            accept_ownership_transfer(&env, &target),
            Err(Error::TransferExpired)
        );
        // Ownership remained with current owner
        assert_eq!(get_admin(&env), current_owner);
        assert_eq!(get_pending_ownership_transfer(&env), None);
    });
}

#[test]
fn test_ownership_transfer_unauthorized_accept_rejected() {
    let (env, contract_id, current_owner) = setup_with_admin_role();
    let target = Address::generate(&env);
    let stranger = Address::generate(&env);
    env.ledger().set_timestamp(100);

    env.as_contract(&contract_id, || {
        propose_ownership_transfer(&env, &current_owner, &target, 200).unwrap();
    });

    env.as_contract(&contract_id, || {
        assert_eq!(
            accept_ownership_transfer(&env, &stranger),
            Err(Error::NotPendingOwner)
        );
        assert_eq!(get_admin(&env), current_owner);
    });
}

#[test]
fn test_ownership_transfer_unauthorized_propose_and_cancel_rejected() {
    let (env, contract_id, current_owner) = setup_with_admin_role();
    let target = Address::generate(&env);
    let stranger = Address::generate(&env);
    env.ledger().set_timestamp(100);

    env.as_contract(&contract_id, || {
        assert_eq!(
            propose_ownership_transfer(&env, &stranger, &target, 200),
            Err(Error::Unauthorized)
        );
        propose_ownership_transfer(&env, &current_owner, &target, 200).unwrap();
    });

    env.as_contract(&contract_id, || {
        assert_eq!(
            cancel_ownership_transfer(&env, &stranger),
            Err(Error::Unauthorized)
        );
    });
}

#[test]
fn test_ownership_transfer_duplicate_rejected() {
    let (env, contract_id, current_owner) = setup_with_admin_role();
    let target1 = Address::generate(&env);
    let target2 = Address::generate(&env);
    env.ledger().set_timestamp(100);

    env.as_contract(&contract_id, || {
        propose_ownership_transfer(&env, &current_owner, &target1, 200).unwrap();
    });

    env.as_contract(&contract_id, || {
        assert_eq!(
            propose_ownership_transfer(&env, &current_owner, &target2, 250),
            Err(Error::TransferAlreadyPending)
        );
    });
}

#[test]
fn test_ownership_transfer_invalid_arguments_rejected() {
    let (env, contract_id, current_owner) = setup_with_admin_role();
    let target = Address::generate(&env);
    env.ledger().set_timestamp(100);

    env.as_contract(&contract_id, || {
        // Expiry in past or equal to now
        assert_eq!(
            propose_ownership_transfer(&env, &current_owner, &target, 100),
            Err(Error::InvalidArgument)
        );
    });

    env.as_contract(&contract_id, || {
        assert_eq!(
            propose_ownership_transfer(&env, &current_owner, &target, 99),
            Err(Error::InvalidArgument)
        );
    });

    env.as_contract(&contract_id, || {
        // Self-transfer
        assert_eq!(
            propose_ownership_transfer(&env, &current_owner, &current_owner, 200),
            Err(Error::InvalidArgument)
        );
    });
}
