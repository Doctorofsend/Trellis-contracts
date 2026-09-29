//! Deterministic Storage Schema Version Guards (Issue #140).
//!
//! Contract storage upgrades require deterministic schema version guards so old
//! or future incompatible ledger data cannot be interpreted incorrectly after a
//! deployment or migration.
//!
//! This module provides:
//! - Explicit schema version storage keys and validation logic.
//! - Strict and backwards-compatible version checking policies.
//! - Deterministic read guards rejecting incompatible schema reads with
//!   [`Error::UnsupportedSchemaVersion`].
//! - Schema upgrade and lazy migration helpers for existing storage entries.
//! - Canonical ledger events for schema validation and migration auditing.

use soroban_sdk::{symbol_short, Env, Symbol};

use crate::errors::Error;
use crate::storage::{instance_get, instance_has, instance_remove, instance_set};

/// Well-known instance storage key for contract storage schema version.
pub const KEY_STORAGE_SCHEMA_VERSION: Symbol = symbol_short!("stor_ver");

/// First schema version ever written to contract storage.
pub const STORAGE_SCHEMA_V1: u32 = 1;
/// Current active schema version.
pub const STORAGE_SCHEMA_V2: u32 = 2;

/// Default current schema version for all newly initialized contracts.
pub const CURRENT_STORAGE_SCHEMA_VERSION: u32 = STORAGE_SCHEMA_V2;
/// Oldest storage schema version still accepted on read paths.
pub const MIN_SUPPORTED_STORAGE_SCHEMA_VERSION: u32 = STORAGE_SCHEMA_V1;
/// Newest storage schema version accepted on read paths (== current).
pub const MAX_SUPPORTED_STORAGE_SCHEMA_VERSION: u32 = CURRENT_STORAGE_SCHEMA_VERSION;

/// Event topic for schema version check.
pub const TOPIC_SCHEMA_CHECK: Symbol = symbol_short!("sch_chk");
/// Event topic for schema version upgrade.
pub const TOPIC_SCHEMA_UPGRADE: Symbol = symbol_short!("sch_upg");

/// Returns the schema version recorded in contract instance storage, or `None`
/// if no schema version has been explicitly initialized.
pub fn get_storage_schema_version(env: &Env) -> Option<u32> {
    instance_get::<Symbol, u32>(env, &KEY_STORAGE_SCHEMA_VERSION)
}

/// Stores or updates the explicit schema version in contract instance storage.
pub fn set_storage_schema_version(env: &Env, version: u32) {
    instance_set(env, &KEY_STORAGE_SCHEMA_VERSION, &version);
}

/// Returns `true` if an explicit storage schema version is set in instance storage.
pub fn has_storage_schema_version(env: &Env) -> bool {
    instance_has(env, &KEY_STORAGE_SCHEMA_VERSION)
}

/// Removes the stored schema version (primarily for testing uninitialized/missing scenarios).
pub fn remove_storage_schema_version(env: &Env) {
    instance_remove(env, &KEY_STORAGE_SCHEMA_VERSION);
}

/// Checks whether a given schema version is supported by the current contract build.
pub fn is_supported_storage_schema(version: u32) -> bool {
    (MIN_SUPPORTED_STORAGE_SCHEMA_VERSION..=MAX_SUPPORTED_STORAGE_SCHEMA_VERSION).contains(&version)
}

/// Checks whether a given schema version is readable but represents a legacy/deprecated layout.
pub fn is_legacy_storage_schema(version: u32) -> bool {
    version < CURRENT_STORAGE_SCHEMA_VERSION && is_supported_storage_schema(version)
}

/// Strict schema version guard: requires that an explicit schema version is recorded
/// and that it falls strictly within the supported version range.
///
/// Returns:
/// - `Ok(version)` if the version is explicitly set and supported.
/// - `Err(Error::NotFound)` if the version entry is completely missing.
/// - `Err(Error::UnsupportedSchemaVersion)` if the version is incompatible (too old or newer than supported).
pub fn ensure_storage_schema_version(env: &Env) -> Result<u32, Error> {
    match get_storage_schema_version(env) {
        Some(version) => {
            if is_supported_storage_schema(version) {
                Ok(version)
            } else {
                Err(Error::UnsupportedSchemaVersion)
            }
        }
        None => Err(Error::NotFound),
    }
}

/// Version-aware storage read guard with configurable policy for missing entries.
///
/// - If `allow_missing_as_legacy` is `true`: an absent schema version is treated
///   as [`STORAGE_SCHEMA_V1`] (old-compatible), allowing existing pre-guard ledger data
///   to be safely read and migrated forward.
/// - If `allow_missing_as_legacy` is `false`: an absent schema version fails immediately
///   with [`Error::UnsupportedSchemaVersion`].
/// - Any incompatible version (e.g. `0` or `> MAX_SUPPORTED_STORAGE_SCHEMA_VERSION`)
///   fails fast with [`Error::UnsupportedSchemaVersion`].
pub fn validate_storage_schema_read(
    env: &Env,
    allow_missing_as_legacy: bool,
) -> Result<u32, Error> {
    match get_storage_schema_version(env) {
        Some(version) => {
            if is_supported_storage_schema(version) {
                env.events().publish(
                    (symbol_short!("storage"), TOPIC_SCHEMA_CHECK),
                    (version, true),
                );
                Ok(version)
            } else {
                env.events().publish(
                    (symbol_short!("storage"), TOPIC_SCHEMA_CHECK),
                    (version, false),
                );
                Err(Error::UnsupportedSchemaVersion)
            }
        }
        None => {
            if allow_missing_as_legacy {
                env.events().publish(
                    (symbol_short!("storage"), TOPIC_SCHEMA_CHECK),
                    (STORAGE_SCHEMA_V1, true),
                );
                Ok(STORAGE_SCHEMA_V1)
            } else {
                env.events().publish(
                    (symbol_short!("storage"), TOPIC_SCHEMA_CHECK),
                    (0u32, false),
                );
                Err(Error::UnsupportedSchemaVersion)
            }
        }
    }
}

/// Standard deterministic guard to be invoked at the top of storage read operations.
/// Rejects missing or incompatible storage states with [`Error::UnsupportedSchemaVersion`].
pub fn guard_storage_read(env: &Env) -> Result<u32, Error> {
    validate_storage_schema_read(env, false)
}

/// Upgrades contract storage schema to a target version, validating forward migration constraints.
///
/// Constraints:
/// - Target version must be supported by this contract build (`<= MAX_SUPPORTED`).
/// - Target version must be strictly greater than current version (monotonic upgrades).
/// - Current version must either be missing (auto-initialized to legacy V1) or supported.
pub fn upgrade_storage_schema(env: &Env, target_version: u32) -> Result<u32, Error> {
    if !is_supported_storage_schema(target_version) {
        return Err(Error::UnsupportedSchemaVersion);
    }

    let current = match get_storage_schema_version(env) {
        Some(v) => {
            if !is_supported_storage_schema(v) {
                return Err(Error::UnsupportedSchemaVersion);
            }
            v
        }
        None => STORAGE_SCHEMA_V1,
    };

    if target_version <= current {
        return Err(Error::InvalidArgument);
    }

    set_storage_schema_version(env, target_version);

    env.events().publish(
        (symbol_short!("storage"), TOPIC_SCHEMA_UPGRADE),
        (current, target_version),
    );

    Ok(target_version)
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{contract, contractimpl};

    #[contract]
    struct DummyContract;

    #[contractimpl]
    impl DummyContract {}

    fn setup() -> (Env, soroban_sdk::Address) {
        let env = Env::default();
        let contract = env.register_contract(None, DummyContract);
        (env, contract)
    }

    #[test]
    fn current_schema_reads_succeed() {
        let (env, contract) = setup();
        env.as_contract(&contract, || {
            set_storage_schema_version(&env, CURRENT_STORAGE_SCHEMA_VERSION);

            assert_eq!(
                get_storage_schema_version(&env),
                Some(CURRENT_STORAGE_SCHEMA_VERSION)
            );
            assert!(is_supported_storage_schema(CURRENT_STORAGE_SCHEMA_VERSION));
            assert!(!is_legacy_storage_schema(CURRENT_STORAGE_SCHEMA_VERSION));

            let res = ensure_storage_schema_version(&env);
            assert_eq!(res, Ok(CURRENT_STORAGE_SCHEMA_VERSION));

            let read_res = guard_storage_read(&env);
            assert_eq!(read_res, Ok(CURRENT_STORAGE_SCHEMA_VERSION));
        });
    }

    #[test]
    fn missing_schema_version_handling() {
        let (env, contract) = setup();
        env.as_contract(&contract, || {
            remove_storage_schema_version(&env);
            assert_eq!(get_storage_schema_version(&env), None);

            // Strict guard rejects missing entry with NotFound
            assert_eq!(ensure_storage_schema_version(&env), Err(Error::NotFound));

            // Strict read guard rejects missing entry with UnsupportedSchemaVersion
            assert_eq!(
                guard_storage_read(&env),
                Err(Error::UnsupportedSchemaVersion)
            );

            // Permissive/legacy migration read treats missing as V1 (old-compatible)
            let legacy_res = validate_storage_schema_read(&env, true);
            assert_eq!(legacy_res, Ok(STORAGE_SCHEMA_V1));
        });
    }

    #[test]
    fn old_compatible_schema_version_succeeds_and_upgrades() {
        let (env, contract) = setup();
        env.as_contract(&contract, || {
            set_storage_schema_version(&env, STORAGE_SCHEMA_V1);

            assert_eq!(get_storage_schema_version(&env), Some(STORAGE_SCHEMA_V1));
            assert!(is_supported_storage_schema(STORAGE_SCHEMA_V1));
            assert!(is_legacy_storage_schema(STORAGE_SCHEMA_V1));

            // Old compatible reads succeed
            let read_res = guard_storage_read(&env);
            assert_eq!(read_res, Ok(STORAGE_SCHEMA_V1));

            // Upgrading to CURRENT succeeds
            let upg_res = upgrade_storage_schema(&env, CURRENT_STORAGE_SCHEMA_VERSION);
            assert_eq!(upg_res, Ok(CURRENT_STORAGE_SCHEMA_VERSION));
            assert_eq!(
                get_storage_schema_version(&env),
                Some(CURRENT_STORAGE_SCHEMA_VERSION)
            );

            // Duplicate or downgrade upgrade is rejected with InvalidArgument
            assert_eq!(
                upgrade_storage_schema(&env, STORAGE_SCHEMA_V1),
                Err(Error::InvalidArgument)
            );
            assert_eq!(
                upgrade_storage_schema(&env, CURRENT_STORAGE_SCHEMA_VERSION),
                Err(Error::InvalidArgument)
            );
        });
    }

    #[test]
    fn incompatible_schema_versions_fail_safely() {
        let (env, contract) = setup();
        env.as_contract(&contract, || {
            // Too old / invalid version 0
            set_storage_schema_version(&env, 0);
            assert!(!is_supported_storage_schema(0));
            assert_eq!(
                ensure_storage_schema_version(&env),
                Err(Error::UnsupportedSchemaVersion)
            );
            assert_eq!(
                guard_storage_read(&env),
                Err(Error::UnsupportedSchemaVersion)
            );
            assert_eq!(
                validate_storage_schema_read(&env, true),
                Err(Error::UnsupportedSchemaVersion)
            );

            // Future / unsupported version 99
            set_storage_schema_version(&env, 99);
            assert!(!is_supported_storage_schema(99));
            assert_eq!(
                ensure_storage_schema_version(&env),
                Err(Error::UnsupportedSchemaVersion)
            );
            assert_eq!(
                guard_storage_read(&env),
                Err(Error::UnsupportedSchemaVersion)
            );
            assert_eq!(
                validate_storage_schema_read(&env, true),
                Err(Error::UnsupportedSchemaVersion)
            );

            // Upgrading to unsupported version 99 is rejected
            assert_eq!(
                upgrade_storage_schema(&env, 99),
                Err(Error::UnsupportedSchemaVersion)
            );
        });
    }
}
