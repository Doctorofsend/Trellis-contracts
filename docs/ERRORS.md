# Error taxonomy and safe rendering

Soroban contract entrypoints return stable numeric contract errors. Their
existing return types remain unchanged so deployed clients do not need an ABI
migration. The shared `error_taxonomy` module converts a contract domain and
numeric error into an `ErrorInfo` containing a stable public code, category,
retryability, user-safe message, recovery guidance, and a 32-byte correlation
ID.

## Rendering an error

Pass the contract family and numeric error code to `describe_error`. Keep the
raw error available only in trusted diagnostics; show users the returned
message and recovery guidance.

```rust
let info = shared::describe_error(
    &env,
    shared::ErrorDomain::Aid,
    raw_contract_error,
    transaction_hash,
);
```

The domain disambiguates contract-local code ranges. Known validation,
authorization, role, oracle, settlement, payment, marketplace, and upgrade
failures have explicit mappings. Unknown codes map to `UNEXPECTED_ERROR` with
non-retryable metadata and generic support guidance; raw internal details are
not included in the user message.

Retryability is conservative. It is true only when a later attempt can
reasonably succeed without changing the submitted request (for example, after
a pause ends, a quota window resets, funds arrive, or an aid record expires).
Validation and authorization errors are not marked retryable. Stale oracle
data is reported as `ORACLE_DATA_STALE`; callers must fetch a fresh quote
instead of retrying the stale value. A payments-domain `SETTLEMENT_TIMEOUT`
means the confirmation was not received; retry the pending settlement rather
than submitting another transfer.

## Correlation IDs and Soroban failures

Supply the 32-byte transaction hash as `correlation_id` when it is available.
An API gateway may instead create a cryptographically random request ID and
retain its mapping to the transaction hash. Include the ID in support
responses and trusted logs.

Failed Soroban invocations roll back storage and ledger events, so a contract
cannot durably record a correlation ID from a failed invocation. The ID must
therefore be attached at the caller/API boundary. This repository contains no
HTTP API or UI; the shared formatter is the integration point for those
boundaries. Contract errors remain the authoritative machine-readable result.

## Versioned numeric catalog

Raw codes are stable within their `ErrorDomain`; clients should persist the
pair `(domain, raw_code)`, not the number alone. The authoritative enum and
canonical client mapping ranges are:

| Domain | Raw-code namespace | Error enum |
|---|---:|---|
| `Shared` | `1-28`, `950-952`, `1000-1008` | `shared::errors::Error` |
| `Aid` | `100-107` | `contracts/aid-contract::AidError` |
| `AccessControl` | `200-214` | `contracts/access-control::AccessControlError` |
| `Oracle` | `500-510` | `contracts/oracle-contract::OracleError` |
| `Payments` | `700-711` | `shared::errors::Error` |
| `Batch` | `800-805` | `shared::batch::BatchError` |
| `Upgradeability` | `900-911` | `contracts/upgradeability::UpgradeError` |
| `Import` | `940-948` | `shared::import::ImportError` |
| `Marketplace` | `2000-2023` | `contracts/nft-marketplace::MarketError` |

Not every number inside a range is assigned. Additive changes require a new
unused raw code, a `describe_error` mapping with conservative retryability, and
an updated range-coverage test. Existing raw values and stable public strings
must not be repurposed. Some shared error variants are legacy aliases: their
numeric values remain valid, while `describe_error` may intentionally map
several equivalent failures to one public code.

The catalog tests assert that every assigned shared error value and every
contract-specific code range has a non-fallback mapping. They also cover all
batch errors, including limit, validation, rollback, empty-input, and
reentrancy outcomes.

## Validation

```bash
cargo test -p shared error_taxonomy
```

Tests cover catalog completeness, stable validation and authorization codes,
settlement retryability, correlation-ID preservation, and the safe fallback
for unexpected errors.
