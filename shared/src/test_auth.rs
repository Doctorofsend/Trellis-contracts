#![cfg(test)]

extern crate std;

use soroban_sdk::{contract, contractimpl, testutils::Address as _, Address, Env};

use crate::{
    auth::{
        get_admin, grant_role, has_permission, has_role, initialize_admin, require_admin,
        require_not_paused, require_permission, require_role, revoke_role, role_for_permission,
        set_admin, Permission, Role,
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
        grant_role(&env, &admin, &service, Role::ServiceActor).unwrap();
        assert!(has_permission(&env, &service, Permission::ServiceOperation));
        assert_eq!(
            require_permission(&env, &service, Permission::ServiceOperation),
            Ok(())
        );
        revoke_role(&env, &admin, &service, Role::ServiceActor).unwrap();
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

        grant_role(&env, &admin, &delegated_admin, Role::Admin).unwrap();
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

// ---------------------------------------------------------------------------
// Authorization matrix: every privileged entrypoint has allow + deny coverage
// ---------------------------------------------------------------------------

#[test]
fn test_set_admin_by_admin_succeeds() {
    let (env, contract_id, admin) = setup();
    let new_admin = Address::generate(&env);
    env.as_contract(&contract_id, || {
        assert_eq!(set_admin(&env, &admin, &new_admin), Ok(()));
        assert_eq!(get_admin(&env), new_admin);
    });
}

#[test]
fn test_set_admin_by_non_admin_fails() {
    let (env, contract_id, admin) = setup();
    let attacker = Address::generate(&env);
    let new_admin = Address::generate(&env);
    env.as_contract(&contract_id, || {
        assert_eq!(set_admin(&env, &attacker, &new_admin), Err(Error::Unauthorized));
        assert_eq!(get_admin(&env), admin);
    });
}

#[test]
fn test_set_admin_by_revoked_admin_fails() {
    let (env, contract_id, admin) = setup();
    let revoked = Address::generate(&env);
    let new_admin = Address::generate(&env);
    env.as_contract(&contract_id, || {
        grant_role(&env, &admin, &revoked, Role::Admin).unwrap();
        revoke_role(&env, &admin, &revoked, Role::Admin).unwrap();
        assert_eq!(set_admin(&env, &revoked, &new_admin), Err(Error::Unauthorized));
        assert_eq!(get_admin(&env), admin);
    });
}

#[test]
fn test_require_permission_denies_actor_without_role() {
    let (env, contract_id, _admin) = setup();
    let actor = Address::generate(&env);
    env.as_contract(&contract_id, || {
        assert_eq!(
            require_permission(&env, &actor, Permission::ManageRoles),
            Err(Error::Unauthorized)
        );
    });
}

#[test]
fn test_require_permission_denies_actor_after_role_revoked() {
    let (env, contract_id, admin) = setup();
    let actor = Address::generate(&env);
    env.as_contract(&contract_id, || {
        grant_role(&env, &admin, &actor, Role::Pauser).unwrap();
        assert_eq!(
            require_permission(&env, &actor, Permission::PauseContracts),
            Ok(())
        );
        revoke_role(&env, &admin, &actor, Role::Pauser).unwrap();
        assert_eq!(
            require_permission(&env, &actor, Permission::PauseContracts),
            Err(Error::Unauthorized)
        );
    });
}

#[test]
fn test_require_role_denies_actor_after_admin_revoked() {
    let (env, contract_id, admin) = setup();
    let actor = Address::generate(&env);
    env.as_contract(&contract_id, || {
        grant_role(&env, &admin, &actor, Role::Upgrader).unwrap();
        assert_eq!(require_role(&env, &actor, Role::Upgrader), Ok(()));
        revoke_role(&env, &admin, &actor, Role::Upgrader).unwrap();
        assert_eq!(
            require_role(&env, &actor, Role::Upgrader),
            Err(Error::Unauthorized)
        );
    });
}
