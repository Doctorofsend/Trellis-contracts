# Deterministic Storage Schema Version Guards (Issue #140)

Contract storage upgrades require deterministic schema version guards so old ledger data cannot be interpreted incorrectly after a deployment or migration.

## Why This Matters

In Soroban, smart contract bytecode upgrades and storage layout updates are decoupled. If a contract is upgraded to new WASM bytecode that expects a different data layout, reading legacy records or reading entries after an incomplete or incompatible migration can lead to state corruption, silent deserialization bugs, or transaction reversions.

The deterministic storage schema version guard layer ensures:
1. **Explicit Schema Tracking**: The contract records its current active storage schema version in instance storage (`stor_ver`).
2. **Deterministic Pre-Read Validation**: Before storage operations read or interpret stored structures, the schema version is verified against supported bounds (`[MIN_SUPPORTED, MAX_SUPPORTED]`).
3. **Fail-Fast Error Handling**: Any unsupported, future, or incompatible schema version causes the read operation to immediately fail with `Error::UnsupportedSchemaVersion`.
4. **Lossless Forward Migration**: Old-compatible versions (e.g. V1) are safely supported on read paths and can be upgraded deterministically to current versions (e.g. V2) via authorized admin functions or migration hooks.

---

## Schema Versions

| Version | Identifier | Description | Status |
|:---:|:---|:---|:---|
| `1` | `STORAGE_SCHEMA_V1` | Initial legacy storage layout without explicit version tag. | Old-compatible (Readable & Upgradable) |
| `2` | `STORAGE_SCHEMA_V2` | Canonical storage layout with explicit instance version guard and audit events. | Current (Active Writes) |

### Version Bounds

- `MIN_SUPPORTED_STORAGE_SCHEMA_VERSION = 1`
- `CURRENT_STORAGE_SCHEMA_VERSION = 2`
- `MAX_SUPPORTED_STORAGE_SCHEMA_VERSION = 2`

---

## Architecture & Storage Layout

### Storage Key
- Key: `symbol_short!("stor_ver")` (Instance Storage)
- Storage Type: `Instance` (Contract-level configuration, survives across ledger closures without TTL eviction risk while contract exists).

### Core Components (`shared::storage_version`)

1. **`get_storage_schema_version(env: &Env) -> Option<u32>`**
   - Retrieves the explicitly stored schema version from instance storage.
   - Returns `None` if uninitialized.

2. **`set_storage_schema_version(env: &Env, version: u32)`**
   - Sets the explicit schema version in instance storage.

3. **`guard_storage_read(env: &Env) -> Result<u32, Error>`**
   - Deterministic read guard executed before storage read operations.
   - Validates that the recorded version falls within `[MIN_SUPPORTED, MAX_SUPPORTED]`.
   - Returns `Err(Error::UnsupportedSchemaVersion)` if incompatible or missing in strict mode.

4. **`validate_storage_schema_read(env: &Env, allow_missing_as_legacy: bool) -> Result<u32, Error>`**
   - Allows configurable handling for legacy pre-guard deployments:
     - When `allow_missing_as_legacy = true`: absent version defaults to `STORAGE_SCHEMA_V1`.
     - When `allow_missing_as_legacy = false`: absent version returns `Err(Error::UnsupportedSchemaVersion)`.

5. **`upgrade_storage_schema(env: &Env, target_version: u32) -> Result<u32, Error>`**
   - Enforces strictly monotonic upgrades: `target_version > current_version`.
   - Ensures `target_version <= MAX_SUPPORTED_STORAGE_SCHEMA_VERSION`.
   - Emits canonical audit event `(storage, sch_upg)`.

---

## Contract Integration (`AidContract`)

In `contracts/aid-contract`:
- `initialize`: Automatically stamps `CURRENT_STORAGE_SCHEMA_VERSION` (2).
- `get_storage_schema_version(env: Env) -> u32`: Public query method returning stored schema version.
- `validate_storage_schema(env: Env) -> Result<u32, AidError>`: Validates that contract storage is readable.
- `get_aid_guarded(env: Env, aid_id: u64) -> Result<Option<CurrentAidRecord>, AidError>`: Version-guarded read that enforces schema compatibility before fetching and formatting aid records.
- `upgrade_storage_schema(env: Env, caller: Address, new_version: u32) -> Result<u32, AidError>`: Admin-only function to upgrade storage schema version.

---

## Ledger Events

The schema version guard emits audit events for off-chain indexers and telemetry:

| Topic | Topics Tuple | Data Tuple | Description |
|:---|:---|:---|:---|
| Schema Check | `(storage, sch_chk)` | `(version: u32, is_supported: bool)` | Emitted when storage schema version is validated. |
| Schema Upgrade | `(storage, sch_upg)` | `(old_version: u32, new_version: u32)` | Emitted when storage schema version is upgraded. |

---

## Acceptance Criteria Verification

The automated test suite in `contracts/aid-contract/src/tests.rs` and `shared/src/storage_version.rs` covers all four acceptance states:

1. **Current Schema Reads**:
   - `test_storage_schema_guard_current_succeeds`: Initialized contract stamps version 2; validation succeeds; guarded record reads succeed with complete data.
2. **Missing Schema Version**:
   - `test_storage_schema_guard_missing_version_fails_safely`: Uninitialized or missing schema versions fail safely with `AidError::UnsupportedSchemaVersion`.
3. **Old-Compatible Schema Version**:
   - `test_storage_schema_guard_old_compatible_version_succeeds_and_upgrades`: Version 1 is recognized as old-compatible; guarded reads succeed; admin can successfully upgrade schema from 1 to 2.
4. **Incompatible Schema Versions**:
   - `test_storage_schema_guard_incompatible_versions_fail_safely`: Out-of-bounds versions (version 0, future version 99) fail safely with `AidError::UnsupportedSchemaVersion`.

---

## Validation Commands

```bash
# Run shared storage version unit tests
cargo test -p shared storage_version

# Run contract-level acceptance test suite
cargo test -p aid-contract test_storage_schema_guard
```
